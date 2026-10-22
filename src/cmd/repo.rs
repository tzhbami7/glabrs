use serde_json::Value;

use crate::cli::repo_cmd::RepoArgs;
use crate::cli::Root;
use crate::cmd::resolve;
use crate::error::Error;

pub fn run(root: &Root, args: &RepoArgs) -> Result<(), Error> {
    match args {
        RepoArgs::View { target, web } => view(root, target.as_deref(), *web),
        RepoArgs::List {
            group,
            per_page,
            page,
            paginate,
        } => list(root, group.as_deref(), *per_page, *page, *paginate),
        RepoArgs::Clone { target, dir } => clone(root, target, dir.as_deref()),
    }
}

/// Resolve project target: explicit > --repo/-R > git remote.
fn project_path(root: &Root, target: Option<&str>) -> Result<String, Error> {
    if let Some(t) = target {
        return Ok(super::mr::encode_project_path(t));
    }
    if let Some(r) = &root.repo {
        return Ok(super::mr::encode_project_path(r));
    }
    let remote = crate::git::detect_remote()?;
    Ok(remote.project_path.replace('/', "%2F"))
}

fn encode(path: &str) -> String {
    super::mr::encode_project_path(path)
}

fn view(root: &Root, target: Option<&str>, web: bool) -> Result<(), Error> {
    let pp = project_path(root, target)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    if web {
        // Prefer the web_url returned by the API (handles path-prefixed
        // instances correctly); fall back to the derived base.
        let url = if let Ok(project) = ctx
            .client()
            .get_json_value(&format!("projects/{pp}"), &[])
        {
            project
                .get("web_url")
                .and_then(|v| v.as_str())
                .map(String::from)
        } else {
            None
        }
        .unwrap_or_else(|| {
            let p = pp.replace("%2F", "/");
            format!("{}/-/project/{p}", ctx.web_url)
        });
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

fn list(
    root: &Root,
    group: Option<&str>,
    per_page: u32,
    page: Option<u32>,
    paginate: bool,
) -> Result<(), Error> {
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut query: Vec<(String, String)> = vec![
        ("per_page".into(), per_page.to_string()),
        ("order_by".into(), "last_activity_at".into()),
    ];
    let path = match group {
        Some(g) => {
            let g_enc = encode(g);
            query.push(("include_subgroups".into(), "true".into()));
            format!("groups/{g_enc}/projects")
        }
        None => "projects".into(),
    };    if let Some(p) = page {
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

/// Open a URL in the browser ($BROWSER > xdg-open).
pub fn open_browser(url: &str) -> Result<(), Error> {
    let browser = std::env::var("BROWSER").unwrap_or_else(|_| "xdg-open".to_string());
    let status = std::process::Command::new(browser)
        .arg(url)
        .status()
        .map_err(|e| Error::runtime(format!("browser: {e}")))?;
    if !status.success() {
        return Err(Error::runtime("browser exited nonzero"));
    }
    Ok(())
}
