use std::sync::Arc;

use crate::api::client::TokenKind;
use crate::cli::Root;
use crate::cmd::Ctx;
use crate::config;
use crate::error::Error;

/// GitLab CI environment: `CI_JOB_TOKEN` + `CI_API_V4_URL` are provided to
/// every job; the job token authenticates project-scoped endpoints only and
/// must be sent as `JOB-TOKEN` (not `PRIVATE-TOKEN`).
struct CiEnv {
    token: String,
    api_url: String,
}

fn detect_ci() -> Option<CiEnv> {
    let token = std::env::var("CI_JOB_TOKEN").ok()?;
    let api_url = std::env::var("CI_API_V4_URL").ok()?;
    if token.is_empty() || api_url.is_empty() {
        return None;
    }
    Some(CiEnv { token, api_url })
}

/// Lowercase a hostname and strip an optional scheme, so that a checkout on
/// `https://Host:443/...` and `auth login --hostname host` look up the same
/// hosts.toml entry. Explicit non-default ports are kept.
///
/// The schemeless remainder is canonicalized by [`canonical_host`]
/// (`src/git`): lowercased, default ports dropped, FQDN trailing dot trimmed.
/// For scheme forms only the authority is kept: any path/query is dropped (a
/// path-prefixed instance must set `api_url` explicitly — a hostname entry
/// never contains a path), and `user:token@host` credentials are stripped so
/// they can never leak into the stored hostname or an error message.
///
/// # Examples
///
/// ```
/// use glabrs::cmd::normalize_host;
///
/// assert_eq!(normalize_host("GitLab.Corp.Example:443"), "gitlab.corp.example");
/// assert_eq!(normalize_host("https://h.example/"), "h.example");
/// // Credentials and path are not part of the host.
/// assert_eq!(normalize_host("https://user:tok@h.example/prefix"), "h.example");
/// // Custom ports are kept distinct.
/// assert_eq!(normalize_host("h.example:8443"), "h.example:8443");
/// ```
#[must_use]
pub fn normalize_host(h: &str) -> String {
    let s = h.trim();
    let authority = if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
    {
        // Authority only: a path-prefixed instance is configured via api_url.
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        &rest[..end]
    } else {
        s
    };
    // `user:token@host`: only the part after the last `@` is the host
    // (`@` cannot appear in a host, so rsplit is safe for userinfo with `@`).
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    crate::git::canonical_host(authority)
}

/// Read the first usable host from the given env vars (in order).
///
/// Empty and whitespace-only values count as unset — the same
/// exported-empty-variable CI artifact [`token_flag`] guards against for
/// `GITLAB_TOKEN`. The survivor is normalized like every host source.
fn env_host(vars: &[&str]) -> Option<String> {
    for var in vars {
        if let Ok(h) = std::env::var(var) {
            let h = h.trim();
            if !h.is_empty() {
                return Some(normalize_host(h));
            }
        }
    }
    None
}

/// Decide which host a command targets when it was detected from the git
/// remote: the detected host itself when it is configured or a token was
/// passed explicitly (the error for an unconfigured host then names the host
/// the user is actually working on); otherwise the single configured host, so
/// checkouts of non-GitLab remotes (github.com & co) still work against the
/// one instance this installation manages. With zero or several configured
/// hosts the detected host wins — guessing among many would silently target
/// the wrong instance, and zero configured hosts is the honest error.
///
/// Pure so the fallback ladder is testable without git or the filesystem.
#[must_use]
pub fn resolve_detected_host(
    detected: &str,
    hosts: &config::hosts::Hosts,
    explicit_token: bool,
) -> String {
    if explicit_token || hosts.get(detected).is_some() {
        detected.to_string()
    } else {
        let names = hosts.names();
        match names.as_slice() {
            [only] => only.clone(),
            _ => detected.to_string(),
        }
    }
}

/// The host a command should target, for reuse outside [`resolve`].
///
/// Same ladder as [`resolve`]: `--hostname` > `GLABRS_HOST` > `GITLAB_HOST` >
/// `CI_API_V4_URL` host > git remote (with the single-configured-host
/// fallback when the remote's host is unconfigured). Loads hosts.toml.
///
/// # Errors
///
/// Returns [`Error`] when hosts.toml cannot be loaded or git detection fails.
pub fn target_hostname(root: &Root) -> Result<String, Error> {
    if let Some(h) = root.hostname.as_deref() {
        return Ok(normalize_host(h));
    }
    if let Some(h) = env_host(&["GLABRS_HOST", "GITLAB_HOST"]) {
        return Ok(h);
    }
    if let Ok(api_url) = std::env::var("CI_API_V4_URL") {
        // host_of_api_url already returns a canonical host.
        if let Some(host) = crate::api::client::host_of_api_url(api_url.trim()) {
            return Ok(host);
        }
    }
    let hosts = config::hosts::Hosts::load()?;
    Ok(resolve_detected_host(
        &crate::git::detect_remote()?.hostname,
        &hosts,
        token_flag(root.token.as_ref()).is_some(),
    ))
}

/// Resolve the target host, merge settings per precedence, and build the client.
///
/// Precedence: flags > env (`GLABRS_HOST` / `GITLAB_HOST` / `GITLAB_TOKEN` / CI) >
/// per-host > global. The CI environment (`CI_JOB_TOKEN` + `CI_API_V4_URL`)
/// applies only when the resolved target host is the CI host; an explicit
/// `--hostname` for a different instance falls through to per-host/global
/// settings instead of receiving the job token.
///
/// # Errors
///
/// Returns [`Error`] when hosts.toml/config.toml cannot be loaded, or when
/// the per-host CA file cannot be read.
///
/// # Panics
///
/// Never: TLS setup failures are impossibility proofs (safe default protocol
/// versions are always available in rustls).
pub fn resolve(root: &Root, hostname_flag: Option<&str>) -> Result<Ctx, Error> {
    warn_insecure(root);
    let hosts = config::hosts::Hosts::load()?;
    let cfg = config::store::Config::load()?;

    // CI detection first: inside a GitLab job, CI_API_V4_URL is authoritative
    // and no stored credentials are needed. The --token flag (clap-populated
    // from GITLAB_TOKEN) still overrides CI. The CI env applies only when the
    // resolved target host is the CI host: an explicit --hostname pointing at
    // another instance must never receive this job's CI_JOB_TOKEN.
    let ci = detect_ci();
    let hostname = match hostname_flag
        .map(normalize_host)
        .or_else(|| env_host(&["GLABRS_HOST", "GITLAB_HOST"]))
    {
        Some(h) => h,
        None => match &ci {
            Some(env) => crate::api::client::host_of_api_url(&env.api_url)
                .unwrap_or_else(|| "localhost".to_string()),
            None => resolve_detected_host(
                &crate::git::detect_remote()?.hostname,
                &hosts,
                token_flag(root.token.as_ref()).is_some(),
            ),
        },
    };
    let ci = ci.filter(|env| {
        crate::api::client::host_of_api_url(&env.api_url).is_none_or(|ci_host| ci_host == hostname)
    });

    let host_cfg = hosts
        .get(&normalize_host(&hostname))
        .cloned()
        .unwrap_or_default();

    // API URL precedence: per-host api_url > CI_API_V4_URL (in CI) > global
    // api_url > https://host[:port]/api/v4 derived from the hostname.
    // normalize_host keeps custom ports in `hostname`, so the port must not
    // be appended a second time (that produced host:port:port, an invalid
    // URL, for any custom-port instance).
    let api_url = host_cfg
        .api_url
        .clone()
        .or_else(|| ci.as_ref().map(|c| c.api_url.clone()))
        .or_else(|| cfg.get_str("api_url"))
        .unwrap_or_else(|| format!("https://{hostname}/api/v4"));

    // Token precedence: --token flag / GITLAB_TOKEN (clap populates the flag
    // via the `env` attribute) > CI_JOB_TOKEN > stored token.
    let token = match token_flag(root.token.as_ref()) {
        Some(t) => Some((t, TokenKind::Personal)),
        None => ci.map(|c| (c.token, TokenKind::Job)),
    }
    .or_else(|| host_cfg.token.clone().map(|t| (t, TokenKind::Personal)));

    let mut headers = host_cfg.headers.clone().unwrap_or_default();
    if let Some(h) = cfg.get("headers") {
        if let Some(table) = h.as_table() {
            for (k, v) in table {
                if let Some(vs) = v.as_str() {
                    // Per-host entries keep their position; the global value
                    // only fills keys the host did not set.
                    headers.entry(k.clone()).or_insert_with(|| vs.to_string());
                }
            }
        }
    }
    validate_headers(&headers)?;

    let settings = crate::api::client::HostSettings { api_url, headers };
    let agent = build_agent(&host_cfg, root.insecure)?;
    let client = crate::api::Client::new(agent, settings, token, root.verbose);

    let web_url = web_base_from_api(&client.api_url, &hostname);

    Ok(Ctx {
        client,
        hostname,
        json: root.json,
        no_pager: root.no_pager,
        web_url,
    })
}

/// Derive the web base from an `api_url`, keeping the scheme (and prefix):
/// `https://host[:port]/prefix/api/v4` -> `https://host[:port]/prefix`.
///
/// Delegates to [`instance_root`], which owns the `/api/v4` boundary. The
/// result is only trusted when it parses as an absolute http(s) URL with a
/// host (`instance_root` echoes unparseable input unchanged); otherwise fall
/// back to `https://host` so garbage never becomes a URL.
fn web_base_from_api(api_url: &str, hostname: &str) -> String {
    let root = crate::api::client::instance_root(api_url);
    let trusted = url::Url::parse(&root)
        .ok()
        .filter(|u| u.host_str().is_some() && matches!(u.scheme(), "http" | "https"));
    match trusted {
        Some(_) => root,
        None => format!("https://{hostname}"),
    }
}

/// Build a ureq agent honoring per-host CA, OS trust roots, and --insecure.
///
/// # Errors
///
/// Returns [`Error`] when the per-host CA file cannot be read or parsed.
pub fn build_agent(host_cfg: &config::HostConfig, insecure: bool) -> Result<ureq::Agent, Error> {
    let tls = match &host_cfg.ca_file {
        Some(ca_path) => {
            let pem = std::fs::read(ca_path)
                .map_err(|e| Error::runtime(format!("ca_file {ca_path}: {e}")))?;
            let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
                rustls_pemfile::certs(&mut pem.as_slice())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| Error::runtime(format!("ca_file {ca_path}: {e}")))?;
            let mut roots = rustls::RootCertStore::empty();
            for cert in certs {
                roots
                    .add(cert)
                    .map_err(|e| Error::runtime(format!("ca_file {ca_path}: {e}")))?;
            }
            client_config_with_roots(roots)
        }
        None => default_client_config(),
    };
    // --insecure overrides certificate checking regardless of the trust roots
    // configured above; the loud warning is printed at dispatch time.
    let tls = if insecure {
        insecure_client_config()
    } else {
        tls
    };

    let builder = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
        .tls_config(Arc::new(tls));
    Ok(builder.build())
}

/// Default trust roots: the OS native certificate store, falling back to the
/// bundled webpki roots when the store is unavailable or empty (bare
/// containers). This fixes corporate proxies with private roots that the OS
/// already trusts.
fn default_client_config() -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = roots.add(cert);
    }
    if let Some(e) = native.errors.first() {
        eprintln!("warning: could not load some OS certificates ({e}); using available roots");
    }
    // Fall back to the bundled webpki roots when the OS store is empty (bare
    // containers), so TLS still works out of the box.
    if roots.is_empty() {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }
    client_config_with_roots(roots)
}

/// Explicit crypto provider: the dependency graph can pull in more than one
/// rustls backend, and `ClientConfig::builder()` panics at runtime when it
/// cannot pick exactly one.
fn crypto_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

fn client_config_with_roots(roots: rustls::RootCertStore) -> rustls::ClientConfig {
    rustls::ClientConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .expect("default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// Certificate verifier that accepts anything. Only reachable with an explicit
/// `--insecure` on the command line; the loud warning is printed at dispatch.
fn insecure_client_config() -> rustls::ClientConfig {
    rustls::ClientConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .expect("default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify))
        .with_no_client_auth()
}

#[derive(Debug)]
struct NoVerify;

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}

/// Validate configured custom header names and values before any request.
///
/// `ureq` writes request headers verbatim to the wire: a `hosts.toml` header
/// containing CR/LF (or a name with `:` or whitespace) would corrupt the
/// request framing, and the failure would surface deep inside the agent as a
/// confusing transport error. Failing at config-load time with the offending
/// header's name turns it into an ordinary usage-time error.
///
/// Rules (loosely RFC 9110, matching what can actually break the wire): the
/// name is nonempty visible ASCII without `:`, space, or tab; the value is a
/// single line of visible ASCII plus spaces/tabs, no leading/trailing
/// whitespace. An empty value is legal HTTP and allowed.
fn validate_headers(headers: &std::collections::BTreeMap<String, String>) -> Result<(), Error> {
    for (k, v) in headers {
        let valid_name = !k.is_empty()
            && k.bytes()
                .all(|b| b.is_ascii_graphic() && b != b':' && b != b' ' && b != b'\t');
        let valid_value = v
            .bytes()
            .all(|b| (0x21..=0x7E).contains(&b) || b == b' ' || b == b'\t')
            && !v.starts_with([' ', '\t'])
            && !v.ends_with([' ', '\t']);
        if !valid_name || !valid_value {
            return Err(Error::usage(format!(
                "invalid custom header {k:?}: names must be visible ASCII \
                 without ':', values a single line without leading/trailing \
                 spaces; fix config.toml or hosts.toml"
            )));
        }
    }
    Ok(())
}

/// Handle --insecure warning at dispatch time.
pub fn warn_insecure(root: &Root) {
    if root.insecure {
        eprintln!("WARNING: --insecure disables TLS certificate verification. Do not use on untrusted networks.");
    }
}

/// Resolve the `--token` flag (clap populates it from `GITLAB_TOKEN` via the
/// `env` attribute) into a usable token value.
///
/// Pure so the empty-value rule is testable without a server: clap captures
/// `env::var_os`, so an empty `GITLAB_TOKEN` (a common CI
/// exported-empty-variable artifact) reaches the flag as `Some("")` — treating
/// that as a token would send an anonymous request instead of falling through
/// to `CI_JOB_TOKEN` or the stored token. Empty means unset.
fn token_flag(flag: Option<&String>) -> Option<String> {
    // One allocation: trim borrows, the owned String is created only for the
    // value that is actually returned.
    flag.map(String::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_host_cases() {
        assert_eq!(normalize_host("GitLab.Corp.Example"), "gitlab.corp.example");
        assert_eq!(
            normalize_host("https://gitlab.corp.example"),
            "gitlab.corp.example"
        );
        assert_eq!(
            normalize_host("http://gitlab.corp.example/"),
            "gitlab.corp.example"
        );
        // Default ports are dropped so both spellings find the same entry.
        assert_eq!(
            normalize_host("gitlab.corp.example:443"),
            "gitlab.corp.example"
        );
        assert_eq!(
            normalize_host("gitlab.corp.example:80"),
            "gitlab.corp.example"
        );
        // Custom ports are kept distinct.
        assert_eq!(
            normalize_host("gitlab.corp.example:8443"),
            "gitlab.corp.example:8443"
        );
        // FQDN trailing dots are trimmed: same host, same entry.
        assert_eq!(
            normalize_host("gitlab.corp.example."),
            "gitlab.corp.example"
        );
    }

    #[test]
    fn web_base_strips_api_suffix() {
        assert_eq!(
            web_base_from_api("https://h.example/gitlab/api/v4", "h.example"),
            "https://h.example/gitlab"
        );
        // No api/v4 suffix: keep the api_url's scheme.
        assert_eq!(
            web_base_from_api("http://h.example:8080/custom", "h.example"),
            "http://h.example:8080"
        );
        assert_eq!(
            web_base_from_api("https://h.example/custom", "h.example"),
            "https://h.example"
        );
        // Unparseable URL: fall back to the default web URL.
        assert_eq!(
            web_base_from_api("not a url", "h.example"),
            "https://h.example"
        );
    }

    #[test]
    fn custom_port_host_derives_valid_api_url_once() {
        // normalize_host keeps custom ports in the hostname, so the derived
        // api_url must contain the port exactly once (the old code parsed
        // the port back out of the hostname and appended it again, yielding
        // host:port:port — an invalid URL).
        let hostname = normalize_host("127.0.0.1:8931");
        assert_eq!(hostname, "127.0.0.1:8931");
        let api_url = format!("https://{hostname}/api/v4");
        assert!(
            url::Url::parse(&api_url).is_ok(),
            "api_url {api_url} must be valid"
        );
        assert_eq!(api_url, "https://127.0.0.1:8931/api/v4");
        // Default ports are still dropped from the hostname.
        assert_eq!(normalize_host("gitlab.example:443"), "gitlab.example");
    }

    #[test]
    fn token_flag_empty_means_unset() {
        // Empty / whitespace-only values fall through to the next source
        // (CI_JOB_TOKEN / stored token); anything else is used as-is.
        assert_eq!(token_flag(Some(&"tok".into())), Some("tok".into()));
        assert_eq!(token_flag(Some(&" tok ".into())), Some("tok".into()));
        assert_eq!(token_flag(Some(&String::new())), None);
        assert_eq!(token_flag(Some(&"   ".into())), None);
        assert_eq!(token_flag(None), None);
    }

    #[test]
    fn normalize_host_strips_credentials_and_path() {
        // Scheme forms keep only the authority: credentials never leak into
        // the stored hostname, and a path is not silently folded into one.
        assert_eq!(
            normalize_host("https://user:tok@GitLab.Corp.Example"),
            "gitlab.corp.example"
        );
        assert_eq!(
            normalize_host("https://git@gitlab.corp.example"),
            "gitlab.corp.example"
        );
        assert_eq!(
            normalize_host("https://gitlab.corp.example/gitlab"),
            "gitlab.corp.example"
        );
        assert_eq!(
            normalize_host("http://user@h.example:8443/prefix?q=1"),
            "h.example:8443"
        );
    }

    #[test]
    fn validate_headers_accepts_normal_and_empty_values() {
        let mut ok = std::collections::BTreeMap::new();
        ok.insert("X-Custom".to_string(), "value".to_string());
        ok.insert("X-Empty".to_string(), String::new());
        ok.insert("Private-Token".to_string(), "a b\tc".to_string());
        assert!(validate_headers(&ok).is_ok());
    }

    #[test]
    fn validate_headers_rejects_framing_breakers() {
        let cases: Vec<(String, String)> = vec![
            // CR/LF anywhere corrupts the request framing.
            ("X-A".into(), "v\r\nHost: evil".into()),
            ("X-A".into(), "v\nHost: evil".into()),
            // A colon in the name splits the header line.
            ("X-A: B".into(), "v".into()),
            // Control bytes and non-ASCII in the value.
            ("X-A".into(), "bad\u{7F}value".into()),
            ("X-A".into(), "bad\u{00E9}value".into()),
            // Whitespace-padded value (would render as an obs-fold risk).
            ("X-A".into(), " padded ".into()),
            // Empty or whitespace-only name.
            (String::new(), "v".into()),
            ("  ".into(), "v".into()),
        ];
        for (k, v) in cases {
            let mut headers = std::collections::BTreeMap::new();
            headers.insert(k, v);
            let err = validate_headers(&headers).expect_err("must reject");
            assert_eq!(err.exit_code(), 2, "case: {err}");
        }
    }

    fn hosts_with(entries: &[&str]) -> config::hosts::Hosts {
        use std::fmt::Write as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hosts.toml");
        let mut text = String::new();
        for name in entries {
            // Writing into the existing String avoids a per-entry allocation;
            // `write!` to a String cannot fail.
            let _ = write!(text, "[\"{name}\"]\ntoken = \"t\"\n");
        }
        std::fs::write(&path, text).expect("write");
        config::hosts::Hosts::load_from(&path).expect("load")
    }

    #[test]
    fn detected_host_wins_when_configured() {
        let hosts = hosts_with(&["gitlab.corp.example"]);
        assert_eq!(
            resolve_detected_host("gitlab.corp.example", &hosts, false),
            "gitlab.corp.example"
        );
    }

    #[test]
    fn unconfigured_remote_falls_back_to_single_configured_host() {
        // A github.com checkout inside an installation that only manages one
        // self-hosted instance still targets that instance.
        let hosts = hosts_with(&["gitlab.corp.example"]);
        assert_eq!(
            resolve_detected_host("github.com", &hosts, false),
            "gitlab.corp.example"
        );
    }

    #[test]
    fn explicit_token_pins_the_detected_host() {
        // --token/GITLAB_TOKEN means the user pointed at this host on purpose;
        // errors must name it, not silently reroute to another instance.
        let hosts = hosts_with(&["gitlab.corp.example"]);
        assert_eq!(
            resolve_detected_host("github.com", &hosts, true),
            "github.com"
        );
    }

    #[test]
    fn zero_or_multiple_configured_hosts_keep_the_detected_host() {
        let none = hosts_with(&[]);
        assert_eq!(
            resolve_detected_host("github.com", &none, false),
            "github.com"
        );

        let many = hosts_with(&["a.example", "b.example"]);
        assert_eq!(
            resolve_detected_host("github.com", &many, false),
            "github.com"
        );
    }
}
