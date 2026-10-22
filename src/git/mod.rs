use std::process::Command;

use crate::error::Error;

/// A parsed git remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub hostname: String,
    /// Project path with leading/trailing slashes and `.git` stripped.
    pub project_path: String,
    /// Non-standard SSH/HTTP port if present.
    pub port: Option<u16>,
}

/// Parse a git remote URL into host + project path.
///
/// Handles: `https://host[:port]/any/depth/owner/repo(.git)`,
/// `ssh://git@host[:port]/...`, and `git@host:owner/repo.git` (SCP syntax).
///
/// The full path is kept: subgroup projects (`group/subgroup/proj`) must not
/// lose their leading segments, and path-prefixed instances are resolved via
/// an explicit `api_url` in config rather than guessed here.
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
                hostname: host,
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
    let path = parsed.path().trim_start_matches('/');
    if path.is_empty() {
        return None;
    }

    Some(Remote {
        hostname: host,
        project_path: clean_path(path),
        port,
    })
}

/// Strip surrounding slashes and a `.git` suffix only when it terminates the
/// final segment (`repo.git` -> `repo`, but `foo.gitbar` stays `foo.gitbar`).
fn clean_path(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    match trimmed.strip_suffix(".git") {
        Some(stem) if !stem.is_empty() && !stem.ends_with('/') => stem.to_string(),
        _ => trimmed.to_string(),
    }
}

/// Read and parse the `origin` remote from the current git checkout.
pub fn detect_remote() -> Result<Remote, Error> {
    let url = run_git(&["remote", "get-url", "origin"])?;
    parse_remote(&url).ok_or_else(|| Error::runtime(format!("cannot parse remote URL: {url}")))
}

/// Run git in the current directory, returning trimmed stdout.
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
        let r = parse_remote("https://gitlab.corp.example:8443/group/proj.git").unwrap();
        assert_eq!(r.hostname, "gitlab.corp.example");
        assert_eq!(r.project_path, "group/proj");
        assert_eq!(r.port, Some(8443));
    }

    #[test]
    fn subgroup_depth_is_preserved() {
        let r = parse_remote("https://gitlab.corp.example:8443/gitlab/group/subgroup/proj.git")
            .unwrap();
        assert_eq!(r.hostname, "gitlab.corp.example");
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
        assert_eq!(r.hostname, "gitlab.corp.example");
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
