use serde_json::Value;

use crate::cmd::Ctx;
use crate::error::Error;

/// Encode an owner/name or owner/subgroup/name path for the API (`/` -> `%2F`),
/// accepting an already-encoded path and a full URL.
///
/// A bare URL with no project path (`https://gitlab.com`) is a usage error:
/// silently encoding it would send `https:%2F%2Fgitlab.com` to the API and
/// surface as a confusing 404.
///
/// Returns the path plus the parsed target host (canonical, like every other
/// host source) so callers can cross-check it against the instance the
/// command resolves to — `-R https://a.example/g/p` against `--hostname
/// b.example` would otherwise query the wrong instance with a valid path.
///
/// # Errors
///
/// Returns [`Error::Usage`] for a bare URL with no project path.
pub fn encode_project_path(s: &str) -> Result<(String, Option<String>), Error> {
    let s = s.trim().trim_matches('/');
    if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
    {
        let Some(path) = rest.split_once('/').map(|(_, p)| p) else {
            return Err(Error::usage(format!(
                "{s} is a hostname, not a project path; use owner/repo (or a full URL with a path)"
            )));
        };
        // Same `.git` rule as remote parsing: strip one terminal
        // `repo.git` suffix, keep repeated or non-terminal ones.
        let host = rest
            .split_once('/')
            .map(|(h, _)| h)
            .map(crate::git::canonical_host);
        return Ok((crate::git::clean_path(path).replace('/', "%2F"), host));
    }
    // Keep any existing %2F encoding intact.
    Ok((s.replace('/', "%2F"), None))
}

/// Resolve the API project path: explicit target > `--repo/-R` > git remote.
///
/// One shared implementation for every command group (commands pass their
/// positional target as `explicit`; everyone else passes `None`). When the
/// default would come from the git remote but the remote's host is not the
/// instance the command targets (see [`crate::cmd::target_hostname`], which
/// may fall back to a single configured host), the remote's path is refused
/// with a clear `-R` hint instead of querying the wrong instance.
///
/// An explicit URL target (`-R https://a.example/g/p`) is cross-checked the
/// same way: its host must match the resolved target instance, so a typo'd
/// `--hostname` cannot quietly query another installation.
///
/// # Errors
///
/// Returns [`Error::Usage`] for a bare URL with no project path or a URL
/// whose host differs from the target instance, the `-R` hint when the
/// checkout's origin does not live on the target instance, and [`Error`]
/// when git remote detection fails.
pub fn project_path(root: &crate::cli::Root, explicit: Option<&str>) -> Result<String, Error> {
    if let Some(t) = explicit.or(root.repo.as_deref()) {
        let (path, url_host) = encode_project_path(t)?;
        if let Some(host) = url_host {
            let target = crate::cmd::target_hostname(root)?;
            if host != target {
                return Err(Error::usage(format!(
                    "-R {t} points at {host} but the target instance is {target}; \
                     pass --hostname {host} or use the bare path"
                )));
            }
        }
        return Ok(path);
    }
    let remote = crate::git::detect_remote()?;
    let target = crate::cmd::target_hostname(root)?;
    if remote.hostname != target {
        return Err(Error::usage(format!(
            "this checkout's origin is {} but the target instance is {target}; pass -R <project> or --hostname",
            remote.hostname
        )));
    }
    Ok(remote.project_path.replace('/', "%2F"))
}

/// The current user's numeric id via `GET /user`.
///
/// # Errors
///
/// Returns [`Error`] when the request fails or the id is missing.
pub fn my_id(ctx: &Ctx) -> Result<u64, Error> {
    let user: Value = ctx.client().get_json_value("user", &[])?;
    user.get("id")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| Error::runtime("cannot resolve current user id"))
}

/// Resolve a user id from a username or a numeric id (`@username` allowed).
///
/// Pure resolver: the `@me` marker is handled by [`user_flag_id`], which
/// gates it on the current context (job tokens cannot call `/user`).
///
/// # Errors
///
/// Returns [`Error::NotFound`] when the username matches no user.
pub fn user_id(ctx: &Ctx, user: &str) -> Result<u64, Error> {
    if let Ok(id) = user.trim_start_matches('@').parse::<u64>() {
        return Ok(id);
    }
    let username = user.trim_start_matches('@');
    let users: Value = ctx
        .client()
        .get_json_value("users", &[("username".into(), username.to_string())])?;
    users
        .as_array()
        .and_then(|a| a.first())
        .and_then(|u| u.get("id").and_then(serde_json::Value::as_u64))
        .ok_or_else(|| Error::not_found(format!("user @{username} not found")))
}

/// Look up a milestone ID by exact title match.
///
/// # Errors
///
/// Returns [`Error::NotFound`] when no milestone has that title.
pub fn milestone_id(ctx: &Ctx, pp: &str, title: &str) -> Result<u64, Error> {
    let milestones: Vec<Value> =
        ctx.client()
            .get_paginated(&format!("projects/{pp}/milestones"), &[], None)?;
    for m in milestones {
        if m.get("title").and_then(|v| v.as_str()) == Some(title) {
            return m
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| Error::runtime("milestone missing id"));
        }
    }
    Err(Error::not_found(format!("milestone {title:?} not found")))
}

/// Resolve a `--description`/`--description-file` pair (both optional) into
/// the text to send: the flag wins when both are given (that combination is
/// otherwise silently resolved by position, which no user can predict), a
/// file is read (stdin via `-`), `None`/`None` means no text.
///
/// Shared by `mr create` and `issue create`.
///
/// # Errors
///
/// Returns [`Error::Usage`] when both sources are given and [`Error`] when
/// the file cannot be read.
pub fn description_text(
    description: Option<&String>,
    description_file: Option<&String>,
) -> Result<Option<String>, Error> {
    match (description, description_file) {
        (Some(_), Some(_)) => Err(Error::usage(
            "--description and --description-file are mutually exclusive",
        )),
        (Some(d), None) => Ok(Some(d.clone())),
        (None, Some(f)) => Ok(Some(crate::render::pager::read_input(f)?)),
        (None, None) => Ok(None),
    }
}

/// Resolve a `--body`/`--body-file` pair (both required-ish): exactly one
/// source must be given.
///
/// Shared by `mr comment` and `issue comment`.
///
/// # Errors
///
/// Returns [`Error::Usage`] when both or neither source is given and
/// [`Error`] when the file cannot be read.
pub fn comment_text(body: Option<&String>, body_file: Option<&String>) -> Result<String, Error> {
    match (body, body_file) {
        (Some(_), Some(_)) => Err(Error::usage(
            "--body and --body-file are mutually exclusive",
        )),
        (Some(b), None) => Ok(b.clone()),
        (None, Some(f)) => crate::render::pager::read_input(f),
        (None, None) => Err(Error::usage("comment requires --body or --body-file")),
    }
}

/// The `--assignee/--reviewer/--author` error for `@me` under CI job tokens.
#[must_use]
pub fn at_me_needs_personal_token(flag: &str) -> Error {
    Error::auth(format!(
        "{flag}=@me needs a personal token (CI job tokens cannot call /user)"
    ))
}

/// Resolve a user flag (`@me`, `@username`, `username`, numeric id) to a
/// numeric id under the current context.
///
/// One shared implementation for every command group: `@me` resolves via
/// `GET /user` (personal tokens only — job tokens fail with exit code 3),
/// numeric ids pass through, anything else is looked up by username.
///
/// # Errors
///
/// Returns [`Error::Auth`] for `@me` under a job token, [`Error::NotFound`]
/// for unknown usernames.
pub fn user_flag_id(ctx: &Ctx, user: &str, flag: &str) -> Result<u64, Error> {
    if user == "@me" {
        return if ctx.can_resolve_current_user() {
            my_id(ctx)
        } else {
            Err(at_me_needs_personal_token(flag))
        };
    }
    user_id(ctx, user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_project_path_variants() {
        assert_eq!(
            encode_project_path("group/proj").unwrap(),
            ("group%2Fproj".to_string(), None)
        );
        assert_eq!(
            encode_project_path("/group/sub/proj/").unwrap(),
            ("group%2Fsub%2Fproj".to_string(), None)
        );
        assert_eq!(
            encode_project_path("group%2Fproj").unwrap(),
            ("group%2Fproj".to_string(), None)
        );
        assert_eq!(
            encode_project_path("https://gitlab.com/group/sub/proj.git").unwrap(),
            (
                "group%2Fsub%2Fproj".to_string(),
                Some("gitlab.com".to_string())
            )
        );
        assert_eq!(encode_project_path("42").unwrap(), ("42".to_string(), None));
        // The URL host is canonicalized like every other host source, so a
        // fancy spelling still matches the plain hosts.toml entry.
        assert_eq!(
            encode_project_path("https://GitLab.Corp.Example:443/group/proj").unwrap(),
            (
                "group%2Fproj".to_string(),
                Some("gitlab.corp.example".to_string())
            )
        );
        // A bare URL has no project path: usage error, not a nonsense
        // encoding like `https:%2F%2Fgitlab.com`.
        let err = encode_project_path("https://gitlab.com").unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("hostname, not a project path"));
    }
}
