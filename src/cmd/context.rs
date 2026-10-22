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
pub fn normalize_host(h: &str) -> String {
    let mut s = h.trim().to_ascii_lowercase();
    if let Some(rest) = s.strip_prefix("https://") {
        s = rest.trim_end_matches('/').to_string();
    } else if let Some(rest) = s.strip_prefix("http://") {
        s = rest.trim_end_matches('/').to_string();
    }
    if let Some((host, port)) = s.rsplit_once(':') {
        let looks_default =
            !host.is_empty() && !port.is_empty() && port.chars().all(|c| c.is_ascii_digit());
        if looks_default && (port == "443" || port == "80") {
            s = host.to_string();
        }
    }
    s
}

/// Resolve the target host, merge settings per precedence, and build the client.
///
/// Precedence: flags > env (GLABRS_HOST / GITLAB_HOST / GITLAB_TOKEN / CI) >
/// per-host > global.
pub fn resolve(root: &Root, hostname_flag: Option<&str>) -> Result<Ctx, Error> {
    warn_insecure(root);
    let hosts = config::hosts::Hosts::load()?;
    let cfg = config::store::Config::load()?;

    // CI detection first: inside a GitLab job, CI_API_V4_URL is authoritative
    // and no stored credentials are needed. GITLAB_TOKEN still overrides.
    let ci = detect_ci();
    let hostname = match hostname_flag
        .map(normalize_host)
        .or_else(|| {
            std::env::var("GLABRS_HOST")
                .ok()
                .map(|ref h| normalize_host(h))
        })
        .or_else(|| {
            std::env::var("GITLAB_HOST")
                .ok()
                .map(|ref h| normalize_host(h))
        }) {
        Some(h) => h,
        None => match &ci {
            Some(env) => crate::api::client::host_of_api_url(&env.api_url)
                .unwrap_or_else(|| "localhost".to_string()),
            None => crate::git::detect_remote()?.hostname,
        },
    };

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

    // Token precedence: --token flag > GITLAB_TOKEN env > CI_JOB_TOKEN > stored.
    let token: Option<(String, TokenKind)> = if let Some(t) = &root.token {
        Some((t.clone(), TokenKind::Personal))
    } else if let Some(t) = std::env::var("GITLAB_TOKEN").ok().filter(|t| !t.is_empty()) {
        Some((t, TokenKind::Personal))
    } else if let Some(c) = &ci {
        Some((c.token.clone(), TokenKind::Job))
    } else {
        host_cfg.token.clone().map(|t| (t, TokenKind::Personal))
    };

    let mut headers = host_cfg.headers.clone().unwrap_or_default();
    if let Some(h) = cfg.get("headers") {
        if let Some(table) = h.as_table() {
            for (k, v) in table {
                if let Some(vs) = v.as_str() {
                    headers.entry(k.clone()).or_insert_with(|| vs.to_string());
                }
            }
        }
    }

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

/// Derive the web base from an api_url:
/// `https://host[:port]/prefix/api/v4` -> `https://host[:port]/prefix`.
fn web_base_from_api(api_url: &str, hostname: &str) -> String {
    let trimmed = api_url.trim_end_matches('/');
    let without_api = match trimmed.strip_suffix("/api/v4") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => format!("https://{hostname}"),
    };
    without_api
}

/// Build a ureq agent honoring per-host CA, OS trust roots, and --insecure.
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
        None => default_client_config()?,
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
fn default_client_config() -> Result<rustls::ClientConfig, Error> {
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
    Ok(client_config_with_roots(roots))
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

/// Handle --insecure warning at dispatch time.
pub fn warn_insecure(root: &Root) {
    if root.insecure {
        eprintln!("WARNING: --insecure disables TLS certificate verification. Do not use on untrusted networks.");
    }
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
    }

    #[test]
    fn web_base_strips_api_suffix() {
        assert_eq!(
            web_base_from_api("https://h.example/gitlab/api/v4", "h.example"),
            "https://h.example/gitlab"
        );
        assert_eq!(
            web_base_from_api("https://h.example/api/v4", "h.example"),
            "https://h.example"
        );
        // No api/v4 suffix: fall back to the default web URL.
        assert_eq!(
            web_base_from_api("https://h.example/custom", "h.example"),
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
}
