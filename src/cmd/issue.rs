use serde_json::{json, Value};

use crate::cli::issue_cmd::IssueArgs;
use crate::cli::{ListOpts, Root};
use crate::cmd::helpers;
use crate::cmd::resolve;
use crate::error::Error;

/// Run one `issue` subcommand.
///
/// # Errors
///
/// Returns [`Error`] when the request fails or a body file cannot be read.
pub fn run(root: &Root, args: &IssueArgs) -> Result<(), Error> {
    match args {
        IssueArgs::List {
            assignee,
            author,
            labels,
            state,
            list: list_opts,
        } => list(
            root,
            assignee.as_ref(),
            author.as_ref(),
            labels.as_ref(),
            state.as_ref(),
            list_opts,
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
            description.as_ref(),
            description_file.as_ref(),
            labels.as_ref(),
            milestone.as_ref(),
            assignee.as_ref(),
        ),
        IssueArgs::View { iid, web } => view(root, *iid, *web),
        IssueArgs::Close { iid } => set_state(root, *iid, "close"),
        IssueArgs::Reopen { iid } => set_state(root, *iid, "reopen"),
        IssueArgs::Comment {
            iid,
            body,
            body_file,
        } => comment(root, *iid, body.as_ref(), body_file.as_ref()),
        IssueArgs::Delete { iid, yes } => delete(root, *iid, *yes),
    }
}

/// Resolve project path: --repo/-R > git remote.
use helpers::project_path;

fn list(
    root: &Root,
    assignee: Option<&String>,
    author: Option<&String>,
    labels: Option<&String>,
    state: Option<&String>,
    opts: &ListOpts,
) -> Result<(), Error> {
    let per_page = opts.per_page;
    let page = opts.page;
    let paginate = opts.paginate;
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut query: Vec<(String, String)> = vec![("per_page".into(), per_page.to_string())];
    if let Some(a) = assignee {
        if a == "@me" {
            query.push((
                "assignee_id".into(),
                helpers::user_flag_id(&ctx, a, "--assignee")?.to_string(),
            ));
        } else {
            query.push((
                "assignee_username".into(),
                a.trim_start_matches('@').to_string(),
            ));
        }
    }
    if let Some(a) = author {
        if a == "@me" {
            query.push((
                "author_id".into(),
                helpers::user_flag_id(&ctx, a, "--author")?.to_string(),
            ));
        } else {
            query.push((
                "author_username".into(),
                a.trim_start_matches('@').to_string(),
            ));
        }
    }
    if let Some(l) = labels {
        query.push(("labels".into(), l.clone()));
    }
    if let Some(s) = state {
        query.push(("state".into(), s.clone()));
    }
    // Pushed unconditionally: honored as the start page under --paginate
    // (get_paginated dedups it) and as the single page otherwise.
    if let Some(p) = page {
        query.push(("page".into(), p.to_string()));
    }
    let items: Vec<Value> = if paginate {
        ctx.client()
            .get_paginated(&format!("projects/{pp}/issues"), &query, None)?
    } else {
        ctx.client()
            .get_json_vec(&format!("projects/{pp}/issues"), &query)?
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
    description: Option<&String>,
    description_file: Option<&String>,
    labels: Option<&String>,
    milestone: Option<&String>,
    assignee: Option<&String>,
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
        let mid = helpers::milestone_id(&ctx, &pp, m)?;
        body["milestone_id"] = Value::Number(mid.into());
    }
    if let Some(a) = assignee {
        body["assignee_ids"] = Value::Array(vec![Value::Number(
            helpers::user_flag_id(&ctx, a, "--assignee")?.into(),
        )]);
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
    // state_event belongs in the body, like every other mutation.
    let issue: Value = ctx.client().send_json_value(
        "PUT",
        &format!("projects/{pp}/issues/{iid}"),
        &[],
        Some(json!({ "state_event": event })),
    )?;
    crate::render::table::render_one(&issue, &["iid", "title", "state"], ctx.json)?;
    Ok(())
}

fn comment(
    root: &Root,
    iid: u64,
    body: Option<&String>,
    body_file: Option<&String>,
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
