use serde_json::{json, Value};

use crate::cli::mr_cmd::MrArgs;
use crate::cli::Root;
use crate::cmd::helpers;
use crate::cmd::resolve;
use crate::error::Error;

pub fn run(root: &Root, args: &MrArgs) -> Result<(), Error> {
    match args {
        MrArgs::List {
            assignee,
            reviewer,
            author,
            state,
            per_page,
            page,
            paginate,
        } => list(
            root, assignee, reviewer, author, state, *per_page, *page, *paginate,
        ),
        MrArgs::Create {
            title,
            description,
            description_file,
            source_branch,
            target_branch,
            draft,
            assignee,
            reviewer,
            milestone,
            labels,
            fill,
            push,
            squash,
            remove_source_branch,
            web,
        } => create(
            root,
            title,
            description,
            description_file,
            source_branch,
            target_branch,
            *draft,
            assignee,
            reviewer,
            milestone,
            labels,
            *fill,
            *push,
            *squash,
            *remove_source_branch,
            *web,
        ),
        MrArgs::View { iid, web } => view(root, *iid, *web),
        MrArgs::Diff { iid } => diff(root, *iid),
        MrArgs::Approve { iid } => approve(root, *iid),
        MrArgs::Merge {
            iid,
            squash,
            remove_source_branch,
            when_pipeline_succeeds,
        } => merge(
            root,
            *iid,
            *squash,
            *remove_source_branch,
            *when_pipeline_succeeds,
        ),
        MrArgs::Close { iid } => set_state(root, *iid, "close"),
        MrArgs::Reopen { iid } => set_state(root, *iid, "reopen"),
        MrArgs::Comment {
            iid,
            body,
            body_file,
        } => comment(root, *iid, body, body_file),
        MrArgs::Ready { iid } => ready(root, *iid),
        MrArgs::Update {
            iid,
            title,
            description,
            assignee,
            reviewer,
            milestone,
            labels,
        } => update(
            root,
            *iid,
            title,
            description,
            assignee,
            reviewer,
            milestone,
            labels,
        ),
    }
}

use helpers::project_path;

/// Encode an owner/name or owner/subgroup/name path for the API (`/` -> `%2F`),
/// accepting an already-encoded path and a full URL.
pub fn encode_project_path(s: &str) -> String {
    let s = s.trim().trim_matches('/');
    if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
    {
        if let Some(path) = rest.split_once('/').map(|(_, p)| p) {
            return path
                .trim_end_matches(".git")
                .trim_end_matches('/')
                .replace('/', "%2F");
        }
    }
    // Keep any existing %2F encoding intact.
    s.replace('/', "%2F")
}

#[allow(clippy::too_many_arguments)]
fn list(
    root: &Root,
    assignee: &Option<String>,
    reviewer: &Option<String>,
    author: &Option<String>,
    state: &Option<String>,
    per_page: u32,
    page: Option<u32>,
    paginate: bool,
) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut query: Vec<(String, String)> = vec![("per_page".into(), per_page.to_string())];
    if let Some(a) = assignee {
        if a == "@me" {
            if ctx.can_resolve_current_user() {
                query.push(("assignee_id".into(), helpers::my_id(&ctx)?.to_string()));
            } else {
                return Err(helpers::at_me_needs_personal_token("--assignee"));
            }
        } else {
            query.push((
                "assignee_username".into(),
                a.trim_start_matches('@').to_string(),
            ));
        }
    }
    if let Some(r) = reviewer {
        if r == "@me" {
            if ctx.can_resolve_current_user() {
                query.push(("reviewer_id".into(), helpers::my_id(&ctx)?.to_string()));
            } else {
                return Err(helpers::at_me_needs_personal_token("--reviewer"));
            }
        } else {
            query.push((
                "reviewer_username".into(),
                r.trim_start_matches('@').to_string(),
            ));
        }
    }
    if let Some(a) = author {
        if a == "@me" {
            if ctx.can_resolve_current_user() {
                query.push(("author_id".into(), helpers::my_id(&ctx)?.to_string()));
            } else {
                return Err(helpers::at_me_needs_personal_token("--author"));
            }
        } else {
            query.push((
                "author_username".into(),
                a.trim_start_matches('@').to_string(),
            ));
        }
    }
    if let Some(s) = state {
        query.push(("state".into(), s.clone()));
    }
    let path = format!("projects/{pp}/merge_requests");
    let items: Vec<Value> = if paginate {
        ctx.client().get_paginated(&path, &query, None)?
    } else {
        let mut q = query;
        if let Some(p) = page {
            q.push(("page".into(), p.to_string()));
        }
        ctx.client().get_json_vec(&path, &q)?
    };
    crate::render::table::render_list(
        &items,
        &[
            "iid",
            "title",
            "state",
            "author.username",
            "source_branch",
            "created_at",
        ],
        ctx.json,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn create(
    root: &Root,
    title: &Option<String>,
    description: &Option<String>,
    description_file: &Option<String>,
    source_branch: &Option<String>,
    target_branch: &Option<String>,
    draft: bool,
    assignee: &Option<String>,
    reviewer: &Option<String>,
    milestone: &Option<String>,
    labels: &Option<String>,
    fill: bool,
    push: bool,
    squash: bool,
    remove_source_branch: bool,
    web: bool,
) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;

    // Push the current branch first when asked. With an explicit
    // --source-branch that is not checked out locally, refuse rather than
    // push the wrong ref (the named branch may not exist here at all).
    let source = match source_branch {
        Some(b) => b.clone(),
        None => current_branch()?,
    };
    if push {
        let here = current_branch()?;
        if here != source {
            return Err(Error::usage(format!(
                "--push pushes the current branch ({here}); it conflicts with --source-branch {source}. Switch branches or drop --push."
            )));
        }
        run_git_push(&source)?;
    }

    let target = match target_branch.clone() {
        Some(t) => t,
        None => default_branch(&ctx, &pp)?,
    };

    // Autodetect title/description from commits when --fill.
    let (mut title, mut description) = match (title, description_file, description) {
        (Some(t), _, _) => (t.clone(), description.clone().unwrap_or_default()),
        (_, Some(f), _) => {
            let d = crate::render::pager::read_input(f)?;
            (
                title
                    .clone()
                    .unwrap_or_else(|| fallback_title(&source, &target)),
                d,
            )
        }
        (None, None, Some(d)) => (fallback_title(&source, &target), d.clone()),
        (None, None, None) => (fallback_title(&source, &target), String::new()),
    };
    if fill {
        let (ft, fd) = fill_from_commits(&source, &target)?;
        title = ft;
        description = fd;
    }
    if draft && !title.to_lowercase().starts_with("draft:") {
        title = format!("Draft: {title}");
    }

    let mut body = json!({
        "source_branch": source,
        "target_branch": target,
        "title": title,
        "description": description,
    });
    if let Some(a) = assignee {
        if a != "@me" {
            body["assignee_ids"] = json!([helpers::user_id(&ctx, a)?]);
        } else if ctx.can_resolve_current_user() {
            body["assignee_ids"] = json!([helpers::my_id(&ctx)?]);
        } else {
            return Err(helpers::at_me_needs_personal_token("--assignee"));
        }
    }
    if let Some(r) = reviewer {
        if r != "@me" {
            body["reviewer_ids"] = json!([helpers::user_id(&ctx, r)?]);
        } else if ctx.can_resolve_current_user() {
            body["reviewer_ids"] = json!([helpers::my_id(&ctx)?]);
        } else {
            return Err(helpers::at_me_needs_personal_token("--reviewer"));
        }
    }
    if let Some(m) = milestone {
        let mid = helpers::milestone_id(&ctx, &pp, m)?;
        body["milestone_id"] = json!(mid);
    }
    if let Some(l) = labels {
        body["labels"] = json!(l);
    }
    // GitLab accepts these at MR creation time; previously the flags were
    // parsed and silently dropped.
    if squash {
        body["squash"] = json!(true);
    }
    if remove_source_branch {
        body["remove_source_branch"] = json!(true);
    }

    let mr: Value = ctx.client().send_json_value(
        "POST",
        &format!("projects/{pp}/merge_requests"),
        &[],
        Some(body),
    )?;
    if web {
        let web_url = mr.get("web_url").and_then(|v| v.as_str()).unwrap_or("");
        if !web_url.is_empty() {
            crate::cmd::repo::open_browser(web_url)?;
            return Ok(());
        }
    }
    crate::render::table::render_one(&mr, &["iid", "title", "web_url"], ctx.json)?;
    Ok(())
}

fn view(root: &Root, iid: u64, web: bool) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mr = fetch_mr(&ctx, &pp, iid)?;
    if web {
        let web_url = mr.get("web_url").and_then(|v| v.as_str()).unwrap_or("");
        if web_url.is_empty() {
            return Err(Error::runtime("MR has no web_url"));
        }
        crate::cmd::repo::open_browser(web_url)?;
        return Ok(());
    }
    crate::render::table::render_one(
        &mr,
        &[
            "iid",
            "title",
            "state",
            "author.username",
            "assignees",
            "reviewers",
            "source_branch",
            "target_branch",
            "description",
            "created_at",
        ],
        ctx.json,
    )?;
    Ok(())
}

fn diff(root: &Root, iid: u64) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let changes: Value = ctx
        .client()
        .get_json_value(&format!("projects/{pp}/merge_requests/{iid}/changes"), &[])?;
    let diffs = changes
        .pointer("/changes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = String::new();
    for d in diffs {
        let old = d.get("old_path").and_then(|v| v.as_str()).unwrap_or("");
        let new = d.get("new_path").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!("diff --git a/{old} b/{new}\n"));
        if let Some(diff) = d.get("diff").and_then(|v| v.as_str()) {
            out.push_str(diff);
            if !diff.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    if ctx.no_pager || out.is_empty() {
        print!("{out}");
    } else {
        crate::render::pager::page(&out, ctx.no_pager)?;
    }
    Ok(())
}

fn approve(root: &Root, iid: u64) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let resp: Value = ctx.client().send_json_value(
        "POST",
        &format!("projects/{pp}/merge_requests/{iid}/approve"),
        &[],
        None,
    )?;
    crate::render::table::render_one(&resp, &["iid", "approved_by"], ctx.json)?;
    Ok(())
}

fn merge(
    root: &Root,
    iid: u64,
    squash: bool,
    remove_source_branch: bool,
    when_pipeline_succeeds: bool,
) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut body = serde_json::Map::new();
    if squash {
        body.insert("squash".into(), json!(true));
    }
    if remove_source_branch {
        body.insert("should_remove_source_branch".into(), json!(true));
    }
    if when_pipeline_succeeds {
        body.insert("merge_when_pipeline_succeeds".into(), json!(true));
    }
    let mr: Value = ctx.client().send_json_value(
        "PUT",
        &format!("projects/{pp}/merge_requests/{iid}/merge"),
        &[],
        Some(Value::Object(body)),
    )?;
    crate::render::table::render_one(&mr, &["iid", "title", "state"], ctx.json)?;
    Ok(())
}

fn set_state(root: &Root, iid: u64, event: &str) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mr: Value = ctx.client().send_json_value(
        "PUT",
        &format!("projects/{pp}/merge_requests/{iid}"),
        &[("state_event".into(), event.to_string())],
        None,
    )?;
    crate::render::table::render_one(&mr, &["iid", "title", "state"], ctx.json)?;
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
        &format!("projects/{pp}/merge_requests/{iid}/notes"),
        &[],
        Some(json!({ "body": text })),
    )?;
    crate::render::table::render_one(&note, &["id", "body"], ctx.json)?;
    Ok(())
}

fn ready(root: &Root, iid: u64) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mr = fetch_mr(&ctx, &pp, iid)?;
    let title = mr.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let stripped = strip_draft_prefix(title);
    if stripped == title {
        if !ctx.json {
            println!("MR #{iid} is already ready");
        }
        return Ok(());
    }
    let updated: Value = ctx.client().send_json_value(
        "PUT",
        &format!("projects/{pp}/merge_requests/{iid}"),
        &[],
        Some(json!({ "title": stripped })),
    )?;
    crate::render::table::render_one(&updated, &["iid", "title", "state"], ctx.json)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn update(
    root: &Root,
    iid: u64,
    title: &Option<String>,
    description: &Option<String>,
    assignee: &Option<String>,
    reviewer: &Option<String>,
    milestone: &Option<String>,
    labels: &Option<String>,
) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut body = serde_json::Map::new();
    if let Some(t) = title {
        body.insert("title".into(), json!(t));
    }
    if let Some(d) = description {
        body.insert("description".into(), json!(d));
    }
    if let Some(a) = assignee {
        if a != "@me" {
            body.insert("assignee_ids".into(), json!([helpers::user_id(&ctx, a)?]));
        } else if ctx.can_resolve_current_user() {
            body.insert("assignee_ids".into(), json!([helpers::my_id(&ctx)?]));
        } else {
            return Err(helpers::at_me_needs_personal_token("--assignee"));
        }
    }
    if let Some(r) = reviewer {
        if r != "@me" {
            body.insert("reviewer_ids".into(), json!([helpers::user_id(&ctx, r)?]));
        } else if ctx.can_resolve_current_user() {
            body.insert("reviewer_ids".into(), json!([helpers::my_id(&ctx)?]));
        } else {
            return Err(helpers::at_me_needs_personal_token("--reviewer"));
        }
    }
    if let Some(m) = milestone {
        body.insert(
            "milestone_id".into(),
            json!(helpers::milestone_id(&ctx, &pp, m)?),
        );
    }
    if let Some(l) = labels {
        body.insert("labels".into(), json!(l));
    }
    let mr: Value = ctx.client().send_json_value(
        "PUT",
        &format!("projects/{pp}/merge_requests/{iid}"),
        &[],
        Some(Value::Object(body)),
    )?;
    crate::render::table::render_one(&mr, &["iid", "title", "state"], ctx.json)?;
    Ok(())
}

fn fetch_mr(ctx: &crate::cmd::Ctx, pp: &str, iid: u64) -> Result<Value, Error> {
    ctx.client()
        .get_json_value(&format!("projects/{pp}/merge_requests/{iid}"), &[])
}

fn current_branch() -> Result<String, Error> {
    crate::git::run_git(&["rev-parse", "--abbrev-ref", "HEAD"])
}

fn run_git_push(branch: &str) -> Result<(), Error> {
    let status = std::process::Command::new("git")
        .args(["push", "-u", "origin", branch])
        .status()
        .map_err(|e| Error::runtime(format!("git push: {e}")))?;
    if !status.success() {
        return Err(Error::runtime("git push failed"));
    }
    Ok(())
}

fn fallback_title(source: &str, target: &str) -> String {
    format!("{source} into {target}")
}

/// Strip the draft/WIP prefix the way GitLab itself recognizes it
/// (`Draft:`, `WIP:`, `[Draft]`, `(WIP)` — case-insensitive, any casing of
/// the label). Slicing goes through `str::get` so multi-byte titles
/// (e.g. CJK) never panic on a non-char-boundary index.
fn strip_draft_prefix(title: &str) -> String {
    let patterns = ["draft:", "wip:", "[draft]", "(wip)"];
    for p in patterns {
        if title
            .get(..p.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(p))
        {
            return title[p.len()..].trim_start().to_string();
        }
    }
    title.to_string()
}

fn fill_from_commits(source: &str, target: &str) -> Result<(String, String), Error> {
    // Try origin/<target>..origin/<source>, falling back to the local branch
    // range (fresh clones of new branches may not have origin/<source> yet).
    let remote_range = format!("origin/{target}..origin/{source}");
    let local_range = format!("{target}..{source}");
    let log = crate::git::run_git(&["log", "--pretty=format:%s%n%b", &remote_range])
        .or_else(|_| crate::git::run_git(&["log", "--pretty=format:%s%n%b", &local_range]))
        .unwrap_or_default();
    let mut lines = log.lines();
    let title = lines.next().unwrap_or("Update branch").to_string();
    let body: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    Ok((title, body))
}

fn default_branch(ctx: &crate::cmd::Ctx, pp: &str) -> Result<String, Error> {
    let project: Value = ctx
        .client()
        .get_json_value(&format!("projects/{pp}"), &[])?;
    Ok(project
        .get("default_branch")
        .and_then(|v| v.as_str())
        .unwrap_or("main")
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_project_path_variants() {
        assert_eq!(encode_project_path("group/proj"), "group%2Fproj");
        assert_eq!(
            encode_project_path("/group/sub/proj/"),
            "group%2Fsub%2Fproj"
        );
        assert_eq!(encode_project_path("group%2Fproj"), "group%2Fproj");
        assert_eq!(
            encode_project_path("https://gitlab.com/group/sub/proj.git"),
            "group%2Fsub%2Fproj"
        );
        assert_eq!(encode_project_path("42"), "42");
    }

    #[test]
    fn strip_draft_prefix_variants() {
        assert_eq!(strip_draft_prefix("Draft: Fix bug"), "Fix bug");
        assert_eq!(strip_draft_prefix("draft: Fix bug"), "Fix bug");
        assert_eq!(strip_draft_prefix("WIP: Fix bug"), "Fix bug");
        assert_eq!(strip_draft_prefix("[Draft] Fix bug"), "Fix bug");
        assert_eq!(strip_draft_prefix("(wip) Fix bug"), "Fix bug");
        assert_eq!(strip_draft_prefix("Fix draft: bug"), "Fix draft: bug");
        assert_eq!(strip_draft_prefix("Fix bug"), "Fix bug");
        // Multi-byte titles must not panic on a mid-codepoint byte index.
        assert_eq!(strip_draft_prefix("日本語abc"), "日本語abc");
        assert_eq!(strip_draft_prefix("日本語WIP: tail"), "日本語WIP: tail");
        assert_eq!(strip_draft_prefix("Ünïcödé: ümlaut"), "Ünïcödé: ümlaut");
        assert_eq!(strip_draft_prefix("WIP: 日本語 tail"), "日本語 tail");
    }
}
