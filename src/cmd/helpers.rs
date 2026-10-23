use serde_json::Value;

use crate::cmd::Ctx;
use crate::error::Error;

/// Resolve the API project path: `--repo/-R` (path or full URL) > git remote.
///
/// One shared implementation so every command group accepts the same
/// spellings: numeric ID, `owner/name`, `group/sub/name`, full URL, and an
/// already-encoded path (`group%2Fname`).
///
/// # Errors
///
/// Returns [`Error::Usage`] for a bare URL with no project path and
/// [`Error`] when git remote detection fails.
pub fn project_path(root: &crate::cli::Root) -> Result<String, Error> {
    if let Some(r) = &root.repo {
        return crate::cmd::mr::encode_project_path(r);
    }
    Ok(crate::git::detect_remote()?
        .project_path
        .replace('/', "%2F"))
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
