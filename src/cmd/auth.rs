use crate::cli::auth_cmd::AuthArgs;
use crate::cli::Root;
use crate::config::Hosts;
use crate::error::Error;
use crate::render::pager::read_input;

/// Run one `auth` subcommand (login validates the token before storing).
///
/// # Errors
///
/// Returns [`Error::Auth`] for invalid tokens and [`Error`] for I/O or
/// request failures.
pub fn run(root: &Root, args: &AuthArgs) -> Result<(), Error> {
    match args {
        AuthArgs::Login {
            hostname,
            token,
            ca_file,
            api_url,
        } => login(
            root,
            hostname.as_ref(),
            token.as_ref(),
            ca_file.as_ref(),
            api_url.as_ref(),
        ),
        AuthArgs::Status { hostname } => {
            let hosts = Hosts::load()?;
            status(root, &hosts, hostname.as_ref())
        }
        AuthArgs::Logout { hostname } => logout(root, hostname.as_ref()),
    }
}

fn login(
    root: &Root,
    hostname_flag: Option<&String>,
    token_arg: Option<&String>,
    ca_file: Option<&String>,
    api_url: Option<&String>,
) -> Result<(), Error> {
    let hostname = match hostname_flag
        .cloned()
        .or_else(|| std::env::var("GLABRS_HOST").ok())
        .or_else(|| std::env::var("GITLAB_HOST").ok())
    {
        Some(h) => crate::cmd::normalize_host(&h),
        None => {
            return Err(Error::usage(
                "auth login requires --hostname (or set GLABRS_HOST/GITLAB_HOST)",
            ))
        }
    };

    // Token: --token > stdin (`-`) > hidden prompt.
    let token = match token_arg.map(String::as_str) {
        Some("-") => read_input("-")?.trim().to_string(),
        Some(t) => t.to_string(),
        None => rpassword::prompt_password(format!("Paste token for {hostname}: "))?,
    };
    if token.is_empty() {
        return Err(Error::auth("empty token"));
    }

    let host_cfg = crate::config::HostConfig {
        token: Some(token.clone()),
        api_url: api_url.cloned(),
        ca_file: ca_file.cloned(),
        ..crate::config::HostConfig::default()
    };

    // Validate the token against GET /user before storing.
    let agent = crate::cmd::build_agent(&host_cfg, root.insecure)?;
    if root.insecure {
        eprintln!(
            "WARNING: --insecure disables TLS certificate verification; the token was sent over an unverified connection."
        );
    }
    let settings = crate::api::client::HostSettings {
        api_url: api_url
            .map(String::as_str)
            .map_or_else(|| format!("https://{hostname}/api/v4"), str::to_string),
        headers: std::collections::BTreeMap::new(),
    };
    let client = crate::api::Client::new(
        agent,
        settings,
        Some((token.clone(), crate::api::client::TokenKind::Personal)),
        root.verbose,
    );
    let user: serde_json::Value = client
        .get_json_value("/user", &[])
        .map_err(|e| Error::auth(format!("token validation failed: {e}")))?;
    let username = user.get("username").and_then(|v| v.as_str()).unwrap_or("?");

    let mut hosts = Hosts::load()?;
    let entry = hosts.entry(&hostname);
    entry.token = Some(token);
    if let Some(u) = api_url {
        entry.api_url = Some(u.clone());
    }
    if let Some(c) = ca_file {
        entry.ca_file = Some(c.clone());
    }
    hosts.save()?;

    if root.json {
        crate::render::table::print_json(&user)?;
    } else {
        println!("Authenticated as {username} on {hostname}");
    }
    Ok(())
}

/// The host to report when the user narrows the scope: `--hostname` beats
/// `GLABRS_HOST` beats `GITLAB_HOST` (same precedence as `login`). `None`
/// means "report every configured host".
///
/// Reads the env vars for the invocation and delegates the pure ladder to
/// [`explicit_target_from`], which is what tests exercise (no process-global
/// mutation).
fn explicit_target(hostname_flag: Option<&String>) -> Option<String> {
    explicit_target_from(
        hostname_flag,
        std::env::var("GLABRS_HOST").ok(),
        std::env::var("GITLAB_HOST").ok(),
    )
}

/// Pure precedence ladder: flag > `GLABRS_HOST` > `GITLAB_HOST`.
fn explicit_target_from(
    hostname_flag: Option<&String>,
    glabrs_host: Option<String>,
    gitlab_host: Option<String>,
) -> Option<String> {
    hostname_flag
        .cloned()
        .or(glabrs_host)
        .or(gitlab_host)
        .map(|h| crate::cmd::normalize_host(&h))
}

fn status(root: &Root, hosts: &Hosts, hostname_flag: Option<&String>) -> Result<(), Error> {
    let targets = if let Some(host) = explicit_target(hostname_flag) {
        vec![host]
    } else {
        let names = hosts.names();
        if names.is_empty() {
            return Err(Error::auth(
                "no hosts configured; run `glabrs auth login --hostname <host>` first",
            ));
        }
        names
    };

    // An explicitly named host introspects exactly that host (`--json` keeps
    // printing the raw `/user` object). A sweep reports every configured host
    // and only fails at the end if any check failed.
    if targets.len() == 1 {
        let report = probe(root, hosts, &targets[0]);
        match report.result {
            Ok(user) => {
                if root.json {
                    crate::render::table::print_json(&user)?;
                } else {
                    print!(
                        "{}",
                        status_summary(&report.hostname, &user, &report.api_url)
                    );
                }
                Ok(())
            }
            Err(e) => Err(e),
        }
    } else {
        let mut reports: Vec<HostReport> = Vec::with_capacity(targets.len());
        for hostname in &targets {
            reports.push(probe(root, hosts, hostname));
        }
        if root.json {
            let json: Vec<serde_json::Value> = reports
                .iter()
                .map(|r| match &r.result {
                    Ok(user) => serde_json::json!({
                        "host": r.hostname,
                        "api_url": r.api_url,
                        "user": user,
                    }),
                    Err(e) => serde_json::json!({
                        "host": r.hostname,
                        "api_url": r.api_url,
                        "error": e.to_string(),
                    }),
                })
                .collect();
            crate::render::table::print_json(&serde_json::Value::Array(json))?;
        } else {
            for r in &reports {
                match &r.result {
                    Ok(user) => {
                        print!("{}", status_summary(&r.hostname, user, &r.api_url));
                    }
                    Err(e) => println!("{}\n  ✗ {e}", r.hostname),
                }
            }
        }
        let failures = reports.iter().filter(|r| r.result.is_err()).count();
        if failures > 0 {
            return Err(Error::auth(format!(
                "{failures} of {} host(s) failed auth checks",
                reports.len()
            )));
        }
        Ok(())
    }
}

/// One host's status outcome: endpoint plus either the `/user` payload or the
/// failure (missing token, unreachable instance, rejected token).
struct HostReport {
    hostname: String,
    api_url: String,
    result: Result<serde_json::Value, Error>,
}

/// Check one configured host: stored token plus a live `GET /user`.
fn probe(root: &Root, hosts: &Hosts, hostname: &str) -> HostReport {
    let entry = hosts.get(hostname);
    let stored = entry.and_then(|h| h.token.clone());
    let api_url = entry
        .and_then(|h| h.api_url.clone())
        .unwrap_or_else(|| format!("https://{hostname}/api/v4"));

    let Some(token) = stored else {
        return HostReport {
            hostname: hostname.to_string(),
            api_url,
            result: Err(Error::auth(format!("no token stored for {hostname}"))),
        };
    };

    let outcome = (|| -> Result<serde_json::Value, Error> {
        let host_cfg = entry.cloned().unwrap_or_default();
        let agent = crate::cmd::build_agent(&host_cfg, root.insecure)?;
        let settings = crate::api::client::HostSettings {
            api_url: api_url.clone(),
            headers: std::collections::BTreeMap::new(),
        };
        let client = crate::api::Client::new(
            agent,
            settings,
            Some((token, crate::api::client::TokenKind::Personal)),
            root.verbose,
        );
        client
            .get_json_value("/user", &[])
            .map_err(|e| Error::auth(format!("token check failed: {e}")))
    })();

    HostReport {
        hostname: hostname.to_string(),
        api_url,
        result: outcome,
    }
}

/// The human-readable per-host block: host header plus indented checks.
fn status_summary(hostname: &str, user: &serde_json::Value, api_url: &str) -> String {
    let username = user
        .get("username")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    let name = user
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("?");
    format!(
        "{hostname}\n  ✓ Logged in as {username} ({name})\n  ✓ api_url: {api_url}\n  ! token stored in plaintext in hosts.toml\n"
    )
}

/// Remove stored credentials for the target host.
///
/// The host is resolved with the shared ladder ([`target_hostname`]) when the
/// subcommand's `--hostname` is absent: `--hostname` > `GLABRS_HOST` >
/// `GITLAB_HOST` > CI > git remote (with the single-configured-host fallback),
/// matching every other command. The subcommand's own flag is passed through
/// explicitly (not via the global `Root.hostname`), so its value can never be
/// lost to clap argument-merging behavior.
fn logout(root: &Root, hostname_flag: Option<&String>) -> Result<(), Error> {
    let mut hosts = Hosts::load()?;
    let hostname = match hostname_flag {
        Some(h) => crate::cmd::normalize_host(h),
        None => crate::cmd::target_hostname(root)?,
    };

    if hosts.remove(&hostname) {
        hosts.save()?;
        if !root.json {
            println!("Removed credentials for {hostname}");
        }
        Ok(())
    } else {
        Err(Error::not_found(format!(
            "no credentials stored for {hostname}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::auth_cmd::AuthArgs as AA;
    use crate::cli::Command;

    fn status_root(hostname: Option<&str>) -> Root {
        Root {
            repo: None,
            hostname: None,
            token: None,
            insecure: false,
            json: false,
            no_pager: true,
            verbose: false,
            command: Command::Auth {
                command: AA::Status {
                    hostname: hostname.map(str::to_string),
                },
            },
        }
    }

    fn user() -> serde_json::Value {
        serde_json::json!({"username": "bot", "name": "GitLab Bot"})
    }

    #[test]
    fn summary_block_has_host_header_and_indented_checks() {
        let out = status_summary("code.swisscom.com", &user(), "https://x/api/v4");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "code.swisscom.com");
        assert_eq!(lines[1], "  ✓ Logged in as bot (GitLab Bot)");
        assert_eq!(lines[2], "  ✓ api_url: https://x/api/v4");
        assert!(lines[3].starts_with("  ! "));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn summary_block_survives_missing_user_fields() {
        let out = status_summary("h", &serde_json::json!({}), "u");
        assert!(out.contains("✓ Logged in as ? (?)"));
    }

    #[test]
    fn empty_hosts_config_is_an_auth_error_with_login_hint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let hosts = Hosts::load_from(&tmp.path().join("hosts.toml")).expect("load");
        let root = status_root(None);
        let err = status(&root, &hosts, None).expect_err("must fail with no hosts");
        assert_eq!(err.exit_code(), 3);
        assert!(err.to_string().contains("auth login --hostname"));
    }

    #[test]
    fn sweep_sums_per_host_outcomes_into_one_auth_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("hosts.toml");
        // Both hosts point at a guaranteed-closed loopback port (root-only can
        // bind port 1), so both probes fail deterministically and hermetically.
        std::fs::write(
            &path,
            "[\"a.example\"]\ntoken = \"t\"\napi_url = \"http://127.0.0.1:1/api/v4\"\n\n[\"b.example\"]\ntoken = \"t\"\napi_url = \"http://127.0.0.1:1/api/v4\"\n",
        )
        .expect("write");
        let hosts = Hosts::load_from(&path).expect("load");
        let root = status_root(None);
        let err = status(&root, &hosts, None).expect_err("probe failures must fail the sweep");
        assert_eq!(err.exit_code(), 3);
        assert_eq!(err.to_string(), "2 of 2 host(s) failed auth checks");
    }

    #[test]
    fn explicit_target_prefers_flag_over_env() {
        // Pure ladder: no process-global env mutation (tests in one binary run
        // on parallel threads).
        let got = explicit_target_from(
            Some(&"from-flag.example".to_string()),
            Some("from-env.example".into()),
            Some("from-gitlab-env.example".into()),
        );
        assert_eq!(got.as_deref(), Some("from-flag.example"));

        let got = explicit_target_from(None, Some("from-env.example".into()), None);
        assert_eq!(got.as_deref(), Some("from-env.example"));

        let got = explicit_target_from(None, None, Some("from-gitlab-env.example".into()));
        assert_eq!(got.as_deref(), Some("from-gitlab-env.example"));

        // Everything empty: sweep every configured host.
        assert_eq!(explicit_target_from(None, None, None), None);

        // Values are normalized like everywhere else.
        let got = explicit_target_from(None, Some("GitLab.Corp.Example:443".into()), None);
        assert_eq!(got.as_deref(), Some("gitlab.corp.example"));
    }
}
