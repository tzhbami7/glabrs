//! Git remote-URL parsing (https, ssh, SCP syntax) and git command helpers.

use std::process::Command;

use crate::error::Error;

/// A parsed git remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// Canonical hostname (see [`canonical_host`]): lowercased, default ports
    /// dropped, non-default ports kept as `host:8443`, trailing FQDN dot
    /// trimmed.
    pub hostname: String,
    /// Project path with leading/trailing slashes and `.git` stripped.
    pub project_path: String,
    /// Non-standard SSH/HTTP port if present.
    pub port: Option<u16>,
}

/// Canonical form of a hostname: lowercased ASCII, default ports (443/80)
/// dropped, non-default ports kept, FQDN trailing dot trimmed. This is the
/// spelling every `hosts.toml` lookup and host comparison uses, so
/// `git@GitLab.Corp.Example:` SCP remotes, `https://Host:8443/...` checkouts,
/// and `auth login --hostname gitlab.corp.example` all resolve to one entry
/// (a custom port is part of the identity on *every* source — flags, env,
/// `CI_API_V4_URL`, `-R` URLs, and git remotes canonicalize identically).
///
/// (The `url` crate lowercases `https://`/`ssh://` hostnames itself; SCP-syntax
/// remotes bypass it entirely, which is why this helper exists.)
#[must_use]
pub fn canonical_host(raw: &str) -> String {
    let s = raw.trim().to_ascii_lowercase();
    // Split a trailing numeric port (rsplit so IPv6 literals `[::1]:8443`
    // survive: only a genuinely numeric last segment counts as a port).
    let (host, port) = match s.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h, Some(p))
        }
        _ => (s.as_str(), None),
    };
    // `gitlab.example.` and `gitlab.example` are the same FQDN. The dot is
    // trimmed from the host part in every branch — a trailing dot followed by
    // a port (`gitlab.example.:8443`) must not hide behind the port suffix.
    let host = host.strip_suffix('.').unwrap_or(host);
    match port {
        // Default ports are dropped: `h.example:443` == `h.example`.
        Some(p) if p == "443" || p == "80" => host.to_string(),
        // Non-default ports are part of the identity on every host source.
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    }
}

/// Parse a git remote URL into host + project path.
///
/// Handles: `https://host[:port]/any/depth/owner/repo(.git)`,
/// `ssh://git@host[:port]/...`, and `git@host:owner/repo.git` (SCP syntax).
/// `hostname` is canonical (see [`canonical_host`]): lowercased, default
/// ports dropped, trailing FQDN dot trimmed.
///
/// The full path is kept: subgroup projects (`group/subgroup/proj`) must not
/// lose their leading segments, and path-prefixed instances are resolved via
/// an explicit `api_url` in config rather than guessed here.
///
/// # Examples
///
/// ```
/// use glabrs::git::parse_remote;
///
/// let r = parse_remote("https://gitlab.com/group/sub/proj.git").unwrap();
/// assert_eq!(r.hostname, "gitlab.com");
/// assert_eq!(r.project_path, "group/sub/proj");
///
/// let scp = parse_remote("git@gitlab.com:group/proj.git").unwrap();
/// assert_eq!(scp.project_path, "group/proj");
/// ```
#[must_use]
pub fn parse_remote(url: &str) -> Option<Remote> {
    let url = url.trim();

    // SCP-like syntax: git@host:owner/repo.git
    if !url.contains("://") {
        if let Some((userinfo_host, path)) = url.split_once(':') {
            let host = userinfo_host.split('@').next_back()?.to_string();
            if host.is_empty() || path.is_empty() {
                return None;
            }
            let path = path.trim_start_matches('/');
            return Some(Remote {
                hostname: canonical_host(&host),
                project_path: clean_path(path),
                port: None,
            });
        }
        return None;
    }

    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_string();
    if host.is_empty() {
        return None;
    }
    let port = parsed.port();

    // Reattach a non-default port before canonicalizing: the stored hostname
    // must match the `hosts.toml` entry `auth login --hostname h:8443` writes
    // (normalize_host keeps non-default ports on every source).
    let host_with_port = match port {
        Some(p) if !matches!(p, 443 | 80) => format!("{host}:{p}"),
        _ => host,
    };

    Some(Remote {
        hostname: canonical_host(&host_with_port),
        project_path: clean_path(path_from(&parsed)?),
        port,
    })
}

/// The URL path without its leading slash; `None` when empty (a bare
/// `https://host` URL names no project).
fn path_from(parsed: &url::Url) -> Option<&str> {
    let path = parsed.path().trim_start_matches('/');
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

/// Strip surrounding slashes and a `.git` suffix only when it terminates the
/// final segment (`repo.git` -> `repo`, but `foo.gitbar` stays `foo.gitbar`).
/// Only one suffix is stripped (`repo.git.git` -> `repo.git`).
#[must_use]
pub(crate) fn clean_path(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    match trimmed.strip_suffix(".git") {
        Some(stem) if !stem.is_empty() && !stem.ends_with('/') => stem.to_string(),
        _ => trimmed.to_string(),
    }
}

/// Read and parse the `origin` remote from the current git checkout.
///
/// # Errors
///
/// Returns [`Error`] when git fails or the URL cannot be parsed.
pub fn detect_remote() -> Result<Remote, Error> {
    let url = run_git(&["remote", "get-url", "origin"])?;
    parse_remote(&url).ok_or_else(|| Error::runtime(format!("cannot parse remote URL: {url}")))
}

/// Run git in the current directory, returning trimmed stdout.
///
/// # Errors
///
/// Returns [`Error`] when git cannot be spawned or exits nonzero (with its
/// stderr in the message).
pub fn run_git(args: &[&str]) -> Result<String, Error> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| Error::runtime(format!("git: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Error::runtime(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_with_port() {
        // Non-default ports are part of the hostname identity (see
        // `custom_port_is_part_of_the_hostname_identity`).
        let r = parse_remote("https://gitlab.corp.example:8443/group/proj.git").unwrap();
        assert_eq!(r.hostname, "gitlab.corp.example:8443");
        assert_eq!(r.project_path, "group/proj");
        assert_eq!(r.port, Some(8443));
    }

    #[test]
    fn custom_port_is_part_of_the_hostname_identity() {
        // Regression: parse_remote used to store the bare host, so a checkout
        // on h.example:8443 missed the hosts.toml entry written by
        // `auth login --hostname h.example:8443` (normalize_host keeps
        // non-default ports). Flag input and remote detection must agree.
        let r = parse_remote("https://h.example:8443/group/proj.git").unwrap();
        assert_eq!(r.hostname, "h.example:8443");
        // SCP syntax has no port field (the first colon starts the path), so
        // a `:8443` there is path, not port — use ssh:// URLs for ports.
        let scp = parse_remote("git@h.example:group/proj.git").unwrap();
        assert_eq!(scp.hostname, "h.example");
        // Default ports are still dropped: same identity as the bare host.
        assert_eq!(
            parse_remote("https://h.example:443/group/proj.git")
                .unwrap()
                .hostname,
            "h.example"
        );
        assert_eq!(
            parse_remote("http://h.example:80/group/proj.git")
                .unwrap()
                .hostname,
            "h.example"
        );
    }

    #[test]
    fn host_of_api_url_keeps_custom_ports() {
        // The CI_API_V4_URL host must land on the same hosts.toml entry as
        // `auth login --hostname h.example:8443`.
        assert_eq!(
            crate::api::client::host_of_api_url("https://h.example:8443/api/v4"),
            Some("h.example:8443".to_string())
        );
        // Default ports still fold away.
        assert_eq!(
            crate::api::client::host_of_api_url("https://h.example:443/api/v4"),
            Some("h.example".to_string())
        );
    }

    #[test]
    fn subgroup_depth_is_preserved() {
        let r = parse_remote("https://gitlab.corp.example:8443/gitlab/group/subgroup/proj.git")
            .unwrap();
        assert_eq!(r.hostname, "gitlab.corp.example:8443");
        assert_eq!(r.project_path, "gitlab/group/subgroup/proj");
        assert_eq!(r.port, Some(8443));
    }

    #[test]
    fn deep_subgroup() {
        let r = parse_remote("https://gitlab.com/a/b/c/d.git").unwrap();
        assert_eq!(r.project_path, "a/b/c/d");
    }

    #[test]
    fn ssh_url() {
        let r = parse_remote("ssh://git@gitlab.corp.example:2222/group/subgroup/proj.git").unwrap();
        assert_eq!(r.hostname, "gitlab.corp.example:2222");
        assert_eq!(r.project_path, "group/subgroup/proj");
        assert_eq!(r.port, Some(2222));
    }

    #[test]
    fn scp_syntax() {
        let r = parse_remote("git@gitlab.corp.example:group/subgroup/proj.git").unwrap();
        assert_eq!(r.hostname, "gitlab.corp.example");
        assert_eq!(r.project_path, "group/subgroup/proj");
    }

    #[test]
    fn scp_syntax_host_is_canonicalized() {
        // SCP URLs bypass the url crate, which lowercases https:// hosts;
        // without canonicalization the raw spelling missed the lowercase
        // hosts.toml entry stored by auth login and commands refused with a
        // spurious -R hint.
        let r = parse_remote("git@GitLab.Corp.Example:group/proj.git").unwrap();
        assert_eq!(r.hostname, "gitlab.corp.example");
        let r = parse_remote("git@host.example.:group/proj.git").unwrap();
        assert_eq!(r.hostname, "host.example");
        // Trailing FQDN dot combined with a port must trim the dot too
        // (normalize_host spellings like `--hostname host.example.:8443`
        // and remote detection must agree).
        let r = parse_remote("ssh://git@host.example.:8443/group/proj.git").unwrap();
        assert_eq!(r.hostname, "host.example:8443");
    }

    #[test]
    fn trailing_fqdn_dot_is_trimmed_for_url_remotes_too() {
        let r = parse_remote("https://gitlab.corp.example./group/proj.git").unwrap();
        assert_eq!(r.hostname, "gitlab.corp.example");
    }

    #[test]
    fn plain_https() {
        let r = parse_remote("https://gitlab.com/group/proj").unwrap();
        assert_eq!(r.hostname, "gitlab.com");
        assert_eq!(r.project_path, "group/proj");
    }

    #[test]
    fn git_suffix_only_strips_terminal_segment() {
        let r = parse_remote("https://gitlab.com/group/proj.gitbar").unwrap();
        assert_eq!(r.project_path, "group/proj.gitbar");
        let r = parse_remote("https://gitlab.com/.git").unwrap();
        assert_eq!(r.project_path, ".git");
    }

    #[test]
    fn http_url_keeps_path() {
        let r = parse_remote("http://gitlab.internal/gitlab/group/proj.git").unwrap();
        assert_eq!(r.project_path, "gitlab/group/proj");
        assert_eq!(r.hostname, "gitlab.internal");
    }
}
