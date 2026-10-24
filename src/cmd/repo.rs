use serde_json::Value;

use crate::cli::repo_cmd::RepoArgs;
use crate::cli::Root;
use crate::cmd::helpers::{self, project_path};
use crate::cmd::resolve;
use crate::error::Error;

/// Run one `repo` subcommand.
///
/// # Errors
///
/// Returns [`Error`] when the request fails or git/browser spawning fails.
pub fn run(root: &Root, args: &RepoArgs) -> Result<(), Error> {
    match args {
        RepoArgs::View { target, web } => view(root, target.as_deref(), *web),
        RepoArgs::List {
            group,
            list: list_opts,
        } => list(root, group.as_deref(), list_opts),
        RepoArgs::Clone { target, dir } => clone(root, target, dir.as_deref()),
    }
}

fn view(root: &Root, target: Option<&str>, web: bool) -> Result<(), Error> {
    let pp = project_path(root, target)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    if web {
        // Prefer the web_url returned by the API (handles path-prefixed
        // instances correctly); fall back to base + encoded path (the
        // project home route). If the API call failed, the fallback is still
        // a real URL instead of a guaranteed 404.
        let url =
            if let Ok(project) = ctx.client().get_json_value(&format!("projects/{pp}"), &[]) {
                project
                    .get("web_url")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            } else {
                None
            }
            .unwrap_or_else(|| format!("{}/{}", ctx.web_url, pp.replace("%2F", "/")));
        open_browser(&url)?;
        return Ok(());
    }
    let project: Value = ctx
        .client()
        .get_json_value(&format!("projects/{pp}"), &[])?;
    crate::render::table::render_one(
        &project,
        &[
            "id",
            "path_with_namespace",
            "description",
            "default_branch",
            "web_url",
            "star_count",
        ],
        ctx.json,
    )?;
    Ok(())
}

fn list(root: &Root, group: Option<&str>, opts: &crate::cli::ListOpts) -> Result<(), Error> {
    let per_page = opts.per_page;
    let page = opts.page;
    let paginate = opts.paginate;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut query: Vec<(String, String)> = vec![
        ("per_page".into(), per_page.to_string()),
        ("order_by".into(), "last_activity_at".into()),
    ];
    let path = match group {
        Some(g) => {
            let (g_enc, _) = helpers::encode_project_path(g)?;
            query.push(("include_subgroups".into(), "true".into()));
            format!("groups/{g_enc}/projects")
        }
        None => "projects".into(),
    };
    if let Some(p) = page {
        query.push(("page".into(), p.to_string()));
    }
    let items: Vec<Value> = if paginate {
        ctx.client().get_paginated(&path, &query, None)?
    } else {
        ctx.client().get_json_vec(&path, &query)?
    };
    crate::render::table::render_list(
        &items,
        &[
            "path_with_namespace",
            "id",
            "description",
            "last_activity_at",
        ],
        ctx.json,
    )?;
    Ok(())
}

fn clone(root: &Root, target: &str, dir: Option<&str>) -> Result<(), Error> {
    let ctx = resolve(root, root.hostname.as_deref())?;
    let url = if target.contains("://") || target.contains('@') {
        target.to_string()
    } else {
        let pp = project_path(root, Some(target))?;
        format!("{}/{}.git", ctx.web_url, pp.replace("%2F", "/"))
    };
    let mut cmd = std::process::Command::new("git");
    cmd.args(["clone", &url]);
    if let Some(d) = dir {
        cmd.arg(d);
    }
    let status = cmd
        .status()
        .map_err(|e| Error::runtime(format!("git clone: {e}")))?;
    if !status.success() {
        return Err(Error::runtime("git clone failed"));
    }
    Ok(())
}

/// Open a URL in the browser per the XDG convention: `$BROWSER` is a
/// colon-separated list of commands tried in order, `%s` (and `%%`) are
/// substituted, a command without `%s` gets the URL appended. Falls back to
/// `xdg-open` when unset or empty. The first command that spawns and exits
/// successfully wins; spawn failures (missing binary) and nonzero exits fall
/// through to the next candidate.
///
/// # Errors
///
/// Returns [`Error`] when every candidate fails to spawn or exits nonzero.
pub fn open_browser(url: &str) -> Result<(), Error> {
    let raw = std::env::var("BROWSER").unwrap_or_else(|_| "xdg-open".to_string());
    let mut last: Option<Error> = None;
    for field in raw.split(':') {
        let field = field.trim();
        if field.is_empty() {
            continue; // "cmd1::cmd2" and a trailing colon are tolerated
        }
        let (program, args): (&str, Vec<&str>) =
            if let Some((before, after)) = field.split_once("%s") {
                let mut args: Vec<&str> = before.split_whitespace().collect();
                args.extend(after.split_whitespace());
                (args[0], args[1..].to_vec())
            } else {
                let mut parts = field.split_whitespace();
                let Some(program) = parts.next() else {
                    continue;
                };
                (program, parts.collect())
            };
        // %% is an escaped literal %, applied after the %s split.
        let args: Vec<String> = args.into_iter().map(|a| a.replace("%%", "%")).collect();
        let status = std::process::Command::new(program)
            .args(&args)
            .arg(url)
            .status();
        match status {
            Ok(st) if st.success() => return Ok(()),
            Ok(st) => last = Some(Error::runtime(format!("browser {program} exited {st}"))),
            Err(e) => last = Some(Error::runtime(format!("browser {program}: {e}"))),
        }
    }
    Err(last.unwrap_or_else(|| Error::runtime("no browser configured ($BROWSER is empty)")))
}
