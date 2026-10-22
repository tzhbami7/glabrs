use serde_json::{json, Value};

use crate::cli::issue_cmd::IssueArgs;
use crate::cli::Root;
use crate::cmd::mr::encode_project_path;
use crate::cmd::resolve;
use crate::error::Error;

pub fn run(root: &Root, args: &IssueArgs) -> Result<(), Error> {
    match args {
        IssueArgs::List {
            assignee,
            author,
            labels,
            state,
            per_page,
            page,
            paginate,
        } => list(
            root, assignee, author, labels, state, *per_page, *page, *paginate,
        ),
        IssueArgs::Create {
            title,
            description,
            description_file,
            labels,
            milestone,
            assignee,
        } => create(
            root,
            title,
            description,
            description_file,
            labels,
            milestone,
            assignee,
        ),
        IssueArgs::View { iid, web } => view(root, *iid, *web),
        IssueArgs::Close { iid } => set_state(root, *iid, "close"),
        IssueArgs::Reopen { iid } => set_state(root, *iid, "reopen"),
        IssueArgs::Comment {
            iid,
            body,
            body_file,
        } => comment(root, *iid, body, body_file),
        IssueArgs::Delete { iid, yes } => delete(root, *iid, *yes),
    }
}

/// Resolve project path: --repo/-R > git remote.
fn project_path(root: &Root) -> Result<String, Error> {
    if let Some(r) = &root.repo {
        return Ok(encode_project_path(r));
    }
    Ok(crate::git::detect_remote()?
        .project_path
        .replace('/', "%2F"))
}

#[allow(clippy::too_many_arguments)]
fn list(
    root: &Root,
    assignee: &Option<String>,
    author: &Option<String>,
    labels: &Option<String>,
    state: &Option<String>,
    per_page: u32,
    page: Option<u32>,
    paginate: bool,
) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut query: Vec<(String, String)> = vec![("per_page".into(), per_page.to_string())];
    if let Some(a) = assignee {
        query.push((
            "assignee_username".into(),
            a.trim_start_matches('@').to_string(),
        ));
    }
    if let Some(a) = author {
        query.push((
            "author_username".into(),
            a.trim_start_matches('@').to_string(),
        ));
    }
    if let Some(l) = labels {
        query.push(("labels".into(), l.clone()));
    }
    if let Some(s) = state {
        query.push(("state".into(), s.clone()));
    }
    let items: Vec<Value> = if paginate {
        ctx.client()
            .get_paginated(&format!("projects/{pp}/issues"), &query, None)?
    } else {
        let mut q = query;
        if let Some(p) = page {
            q.push(("page".into(), p.to_string()));
        }
        ctx.client()
            .get_json_vec(&format!("projects/{pp}/issues"), &q)?
    };
    crate::render::table::render_list(
        &items,
        &[
            "iid",
            "title",
            "state",
            "author.username",
            "labels",
            "created_at",
        ],
        ctx.json,
    )?;
    Ok(())
}

fn create(
    root: &Root,
    title: &str,
    description: &Option<String>,
    description_file: &Option<String>,
    labels: &Option<String>,
    milestone: &Option<String>,
    assignee: &Option<String>,
) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut body = json!({ "title": title });
    if let Some(d) = description_file {
        body["description"] = Value::String(crate::render::pager::read_input(d)?);
    } else if let Some(d) = description {
        body["description"] = Value::String(d.clone());
    }
    if let Some(l) = labels {
        body["labels"] = Value::String(l.clone());
    }
    if let Some(m) = milestone {
        let mid = milestone_id(&ctx, &pp, m)?;
        body["milestone_id"] = Value::Number(mid.into());
    }
    if let Some(a) = assignee {
        let uid = if a == "@me" {
            if ctx.can_resolve_current_user() {
                my_id_num(&ctx)?
            } else {
                return Err(Error::auth(
                    "--assignee=@me needs a personal token (CI job tokens cannot call /user)",
                ));
            }
        } else {
            user_id(&ctx, a)?
        };
        body["assignee_ids"] = Value::Array(vec![Value::Number(uid.into())]);
    }
    let issue: Value =
        ctx.client()
            .send_json_value("POST", &format!("projects/{pp}/issues"), &[], Some(body))?;
    crate::render::table::render_one(&issue, &["iid", "title", "web_url"], ctx.json)?;
    Ok(())
}

fn view(root: &Root, iid: u64, web: bool) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    if web {
        let project: Value = ctx
            .client()
            .get_json_value(&format!("projects/{pp}"), &[])?;
        let web_url = project
            .get("web_url")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let url = format!("{web_url}/-/issues/{iid}");
        crate::cmd::repo::open_browser(&url)?;
        return Ok(());
    }
    let issue: Value = ctx
        .client()
        .get_json_value(&format!("projects/{pp}/issues/{iid}"), &[])?;
    crate::render::table::render_one(
        &issue,
        &[
            "iid",
            "title",
            "state",
            "author.username",
            "assignees",
            "labels",
            "description",
            "created_at",
            "updated_at",
        ],
        ctx.json,
    )?;
    Ok(())
}

fn set_state(root: &Root, iid: u64, event: &str) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let issue: Value = ctx.client().send_json_value(
        "PUT",
        &format!("projects/{pp}/issues/{iid}"),
        &[("state_event".into(), event.to_string())],
        None,
    )?;
    crate::render::table::render_one(&issue, &["iid", "title", "state"], ctx.json)?;
    Ok(())
}

fn comment(
    root: &Root,
    iid: u64,
    body: &Option<String>,
    body_file: &Option<String>,
) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let text = match body_file {
        Some(p) => crate::render::pager::read_input(p)?,
        None => match body {
            Some(b) => b.clone(),
            None => return Err(Error::usage("comment requires --body or --body-file")),
        },
    };
    let note: Value = ctx.client().send_json_value(
        "POST",
        &format!("projects/{pp}/issues/{iid}/notes"),
        &[],
        Some(json!({ "body": text })),
    )?;
    crate::render::table::render_one(&note, &["id", "body"], ctx.json)?;
    Ok(())
}

fn delete(root: &Root, iid: u64, yes: bool) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    if !yes {
        eprint!("Delete issue #{iid}? [y/N] ");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !line.trim().eq_ignore_ascii_case("y") {
            println!("aborted");
            return Ok(());
        }
    }
    ctx.client()
        .send_json_value("DELETE", &format!("projects/{pp}/issues/{iid}"), &[], None)?;
    if !ctx.json {
        println!("Deleted issue #{iid}");
    }
    Ok(())
}

/// Look up a milestone ID by title (e.g. "v2.1").
fn milestone_id(ctx: &crate::cmd::Ctx, pp: &str, title: &str) -> Result<u64, Error> {
    let milestones: Vec<Value> =
        ctx.client()
            .get_paginated(&format!("projects/{pp}/milestones"), &[], None)?;
    for m in milestones {
        if m.get("title").and_then(|v| v.as_str()) == Some(title) {
            return m
                .get("id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| Error::runtime("milestone missing id"));
        }
    }
    Err(Error::not_found(format!("milestone {title:?} not found")))
}

/// Look up a user ID by username (with or without @, or `@me`).
fn user_id(ctx: &crate::cmd::Ctx, username: &str) -> Result<u64, Error> {
    if username == "@me" {
        return my_id_num(ctx);
    }
    let username = username.trim_start_matches('@');
    let users: Vec<Value> = ctx
        .client()
        .get_json_vec("users", &[("username".into(), username.to_string())])?;
    users
        .first()
        .and_then(|u| u.get("id").and_then(|v| v.as_u64()))
        .ok_or_else(|| Error::not_found(format!("user @{username} not found")))
}

fn my_id_num(ctx: &crate::cmd::Ctx) -> Result<u64, Error> {
    let user: Value = ctx.client().get_json_value("user", &[])?;
    user.get("id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| Error::runtime("cannot resolve current user id"))
}
