use std::fmt::Write as _;

use serde_json::{json, Value};

use crate::cli::mr_cmd::MrArgs;
use crate::cli::{ListOpts, Root};
use crate::cmd::helpers::{self, project_path};
use crate::cmd::resolve;
use crate::error::Error;

/// Run one `mr` subcommand.
///
/// # Errors
///
/// Returns [`Error`] when the request fails or a body file cannot be read.
pub fn run(root: &Root, args: &MrArgs) -> Result<(), Error> {
    match args {
        MrArgs::List {
            assignee,
            reviewer,
            author,
            state,
            list: list_opts,
        } => list(
            root,
            assignee.as_ref(),
            reviewer.as_ref(),
            author.as_ref(),
            state.as_ref(),
            list_opts,
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
            &CreateParams {
                title,
                description,
                description_file,
                source_branch,
                target_branch,
                draft: *draft,
                assignee,
                reviewer,
                milestone,
                labels,
                fill: *fill,
                push: *push,
                squash: *squash,
                remove_source_branch: *remove_source_branch,
                web: *web,
            },
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
        } => comment(root, *iid, body.as_ref(), body_file.as_ref()),
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
            &UpdateParams {
                title,
                description,
                assignee,
                reviewer,
                milestone,
                labels,
            },
        ),
    }
}

fn list(
    root: &Root,
    assignee: Option<&String>,
    reviewer: Option<&String>,
    author: Option<&String>,
    state: Option<&String>,
    opts: &ListOpts,
) -> Result<(), Error> {
    let per_page = opts.per_page;
    let page = opts.page;
    let paginate = opts.paginate;
    let pp = project_path(root, None)?;
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
    if let Some(r) = reviewer {
        if r == "@me" {
            query.push((
                "reviewer_id".into(),
                helpers::user_flag_id(&ctx, r, "--reviewer")?.to_string(),
            ));
        } else {
            query.push((
                "reviewer_username".into(),
                r.trim_start_matches('@').to_string(),
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
    if let Some(s) = state {
        query.push(("state".into(), s.clone()));
    } else {
        // Like glab, list opened MRs by default; GitLab's API itself defaults
        // to `all`, which silently changed what a bare `mr list` means.
        query.push(("state".into(), "opened".into()));
    }
    // Pushed unconditionally: honored as the start page under --paginate
    // (get_paginated dedups it) and as the single page otherwise.
    if let Some(p) = page {
        query.push(("page".into(), p.to_string()));
    }
    let path = format!("projects/{pp}/merge_requests");
    let items: Vec<Value> = if paginate {
        ctx.client().get_paginated(&path, &query, None)?
    } else {
        ctx.client().get_json_vec(&path, &query)?
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

/// Everything `mr create` needs, in one struct so the function signature
/// stays readable (and clippy's `too_many_arguments` stays quiet honestly).
#[expect(
    clippy::struct_excessive_bools,
    reason = "flags mirror the CLI surface 1:1; a state machine would obscure them"
)]
struct CreateParams<'a> {
    title: &'a Option<String>,
    description: &'a Option<String>,
    description_file: &'a Option<String>,
    source_branch: &'a Option<String>,
    target_branch: &'a Option<String>,
    draft: bool,
    assignee: &'a Option<String>,
    reviewer: &'a Option<String>,
    milestone: &'a Option<String>,
    labels: &'a Option<String>,
    fill: bool,
    push: bool,
    squash: bool,
    remove_source_branch: bool,
    web: bool,
}

fn create(root: &Root, p: &CreateParams) -> Result<(), Error> {
    let CreateParams {
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
    } = p;
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;

    // Push the current branch first when asked. With an explicit
    // --source-branch that is not checked out locally, refuse rather than
    // push the wrong ref (the named branch may not exist here at all).
    let source = match source_branch {
        Some(b) => b.clone(),
        None => current_branch()?,
    };
    if *push {
        let here = current_branch()?;
        if here != source {
            return Err(Error::usage(format!(
                "--push pushes the current branch ({here}); it conflicts with --source-branch {source}. Switch branches or drop --push."
            )));
        }
        run_git_push(&source)?;
    }

    let target = match target_branch.as_deref() {
        Some(t) => t.to_string(),
        None => default_branch(&ctx, &pp)?,
    };

    // Fill provenance is decided before any fallback: explicit flags win over
    // --fill, and --fill only fills what the user did not provide. A provided
    // --description-file is explicit provenance even if it turns out empty
    // (an empty file means "no description", not "not provided"), so the fill
    // fallback must not override it.
    let explicit_title = title.is_some();
    let explicit_description = description.is_some() || description_file.is_some();
    let (mut title, mut description) = resolve_title_and_description(
        title.as_ref(),
        description_file.as_ref(),
        description.as_ref(),
        &source,
        &target,
    )?;
    // --fill with nothing left to fill skips the git log spawn entirely.
    if *fill && (!explicit_title || !explicit_description) {
        let (ft, fd) = fill_from_commits(&source, &target);
        if !explicit_title {
            title = ft;
        }
        if !explicit_description {
            description = fd;
        }
    }
    if *draft && !title.to_lowercase().starts_with("draft:") {
        title = format!("Draft: {title}");
    }

    let mut body = json!({
        "source_branch": source,
        "target_branch": target,
        "title": title,
        "description": description,
    });
    if let Some(a) = assignee {
        body["assignee_ids"] = json!([helpers::user_flag_id(&ctx, a, "--assignee")?]);
    }
    if let Some(r) = reviewer {
        body["reviewer_ids"] = json!([helpers::user_flag_id(&ctx, r, "--reviewer")?]);
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
    if *squash {
        body["squash"] = json!(true);
    }
    if *remove_source_branch {
        body["remove_source_branch"] = json!(true);
    }

    let mr: Value = ctx.client().send_json_value(
        "POST",
        &format!("projects/{pp}/merge_requests"),
        &[],
        Some(&body),
    )?;
    if *web {
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
    let pp = project_path(root, None)?;
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
    let pp = project_path(root, None)?;
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
        let _ = writeln!(out, "diff --git a/{old} b/{new}");
        if let Some(diff) = d.get("diff").and_then(|v| v.as_str()) {
            out.push_str(diff);
            if !diff.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    if out.is_empty() {
        return Ok(());
    }
    crate::render::pager::page(&out, ctx.no_pager)?;
    Ok(())
}

fn approve(root: &Root, iid: u64) -> Result<(), Error> {
    let pp = project_path(root, None)?;
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
    let pp = project_path(root, None)?;
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
        Some(&Value::Object(body)),
    )?;
    crate::render::table::render_one(&mr, &["iid", "title", "state"], ctx.json)?;
    Ok(())
}

fn set_state(root: &Root, iid: u64, event: &str) -> Result<(), Error> {
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    // state_event belongs in the body, like every other mutation.
    let mr: Value = ctx.client().send_json_value(
        "PUT",
        &format!("projects/{pp}/merge_requests/{iid}"),
        &[],
        Some(&json!({ "state_event": event })),
    )?;
    crate::render::table::render_one(&mr, &["iid", "title", "state"], ctx.json)?;
    Ok(())
}

fn comment(
    root: &Root,
    iid: u64,
    body: Option<&String>,
    body_file: Option<&String>,
) -> Result<(), Error> {
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let text = helpers::comment_text(body, body_file)?;
    let note: Value = ctx.client().send_json_value(
        "POST",
        &format!("projects/{pp}/merge_requests/{iid}/notes"),
        &[],
        Some(&json!({ "body": text })),
    )?;
    crate::render::table::render_one(&note, &["id", "body"], ctx.json)?;
    Ok(())
}

fn ready(root: &Root, iid: u64) -> Result<(), Error> {
    let pp = project_path(root, None)?;
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
        Some(&json!({ "title": stripped })),
    )?;
    crate::render::table::render_one(&updated, &["iid", "title", "state"], ctx.json)?;
    Ok(())
}

/// Everything `mr update` needs, in one struct (mirrors `CreateParams`).
struct UpdateParams<'a> {
    title: &'a Option<String>,
    description: &'a Option<String>,
    assignee: &'a Option<String>,
    reviewer: &'a Option<String>,
    milestone: &'a Option<String>,
    labels: &'a Option<String>,
}

fn update(root: &Root, iid: u64, p: &UpdateParams) -> Result<(), Error> {
    let UpdateParams {
        title,
        description,
        assignee,
        reviewer,
        milestone,
        labels,
    } = p;
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut body = serde_json::Map::new();
    if let Some(t) = title {
        body.insert("title".into(), json!(t));
    }
    if let Some(d) = description {
        body.insert("description".into(), json!(d));
    }
    if let Some(a) = assignee {
        body.insert(
            "assignee_ids".into(),
            json!([helpers::user_flag_id(&ctx, a, "--assignee")?]),
        );
    }
    if let Some(r) = reviewer {
        body.insert(
            "reviewer_ids".into(),
            json!([helpers::user_flag_id(&ctx, r, "--reviewer")?]),
        );
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
        Some(&Value::Object(body)),
    )?;
    crate::render::table::render_one(&mr, &["iid", "title", "state"], ctx.json)?;
    Ok(())
}

fn fetch_mr(ctx: &crate::cmd::Ctx, pp: &str, iid: u64) -> Result<Value, Error> {
    ctx.client()
        .get_json_value(&format!("projects/{pp}/merge_requests/{iid}"), &[])
}

fn current_branch() -> Result<String, Error> {
    // Detached HEAD reports the literal "HEAD": `--push` would push it while
    // the API call sends source_branch=HEAD and fails confusingly, so refuse
    // with the actual remedy.
    let branch = crate::git::run_git(&["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        return Err(Error::usage(
            "HEAD is detached; check out (or create) a branch before creating an MR",
        ));
    }
    Ok(branch)
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

/// Decide the MR title and description: explicit flags win over `--fill`, and
/// `--fill` only fills what the user did not provide. A provided
/// `--description-file` is explicit provenance even if it turns out empty (an
/// empty file means "no description", not "not provided"), so the fill
/// fallback must not override it — and it is read even when `--title` is
/// given: `--title X --description-file f` must not silently drop `f`.
fn resolve_title_and_description(
    title: Option<&String>,
    description_file: Option<&String>,
    description: Option<&String>,
    source: &str,
    target: &str,
) -> Result<(String, String), Error> {
    let description = helpers::description_text(description, description_file)?;
    let title = match title {
        Some(t) => t.clone(),
        None => fallback_title(source, target),
    };
    Ok((title, description.unwrap_or_default()))
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

/// Collect the commit subjects/bodies feeding the MR as (title, description).
///
/// Tries `origin/<target>..origin/<source>`, falling back to the local branch
/// range (fresh clones of new branches may not have origin/<source> yet).
/// The title is the *newest* commit's subject (`-1 --reverse` takes the last
/// commit of the range), matching GitHub/glab convention — a multi-commit MR
/// titled from the oldest ("bottom") commit reads upside down. An empty
/// result falls back to "Update branch" with an empty description.
fn fill_from_commits(source: &str, target: &str) -> (String, String) {
    let remote_range = format!("origin/{target}..origin/{source}");
    let local_range = format!("{target}..{source}");
    let log = crate::git::run_git(&[
        "log",
        "--reverse",
        "--pretty=format:%s%n%b%n%x00",
        "-1",
        &remote_range,
    ])
    .or_else(|_| {
        crate::git::run_git(&[
            "log",
            "--reverse",
            "--pretty=format:%s%n%b%n%x00",
            "-1",
            &local_range,
        ])
    })
    .unwrap_or_default();
    parse_fill_log(&log)
}

/// Pure: turn one `git log --reverse --pretty=format:%s%n%b%n%x00 -1` record
/// into (title, description). Empty output (git failed, or the range has no
/// commits) falls back to "Update branch" with an empty description.
///
/// Separated from the git-spawning wrapper so the parsing rules are testable
/// without redirecting the process cwd.
fn parse_fill_log(log: &str) -> (String, String) {
    let record = log.split('\0').next().unwrap_or("").trim();
    if record.is_empty() {
        return ("Update branch".to_string(), String::new());
    }
    let mut parts = record.splitn(2, '\n');
    let title = parts.next().unwrap_or("Update branch").to_string();
    let body = parts.next().unwrap_or("").trim().to_string();
    (title, body)
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
    fn parse_fill_log_takes_the_newest_record() {
        // `--reverse -1` yields exactly one record; the parser still takes
        // only the first (defensive against a future format change).
        let log = "newest subject\nnewest body\n\n\0old record would follow\0";
        let (title, body) = parse_fill_log(log);
        assert_eq!(title, "newest subject");
        assert_eq!(body, "newest body");

        // Subject without a body.
        let (title, body) = parse_fill_log("only subject\n\n\0");
        assert_eq!(title, "only subject");
        assert_eq!(body, "");

        // Empty output (git failed or an empty range): the documented fallback.
        let (title, body) = parse_fill_log("");
        assert_eq!(title, "Update branch");
        assert_eq!(body, "");
    }

    #[test]
    fn fallback_title_format() {
        assert_eq!(fallback_title("feat", "main"), "feat into main");
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

    fn desc_file(dir: &tempfile::TempDir, content: &str) -> String {
        let path = dir.path().join("desc.md");
        std::fs::write(&path, content).expect("write");
        path.to_string_lossy().to_string()
    }

    #[test]
    fn title_with_description_file_reads_the_file() {
        // `--title X --description-file f` must honor f (the old code matched
        // any --title first and silently dropped the file's content).
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = desc_file(&tmp, "full\nnotes");
        let (title, desc) =
            resolve_title_and_description(Some(&"T".into()), Some(&f), None, "feat", "main")
                .expect("resolve");
        assert_eq!(title, "T");
        assert_eq!(desc, "full\nnotes");
    }

    #[test]
    fn title_with_description_flag_keeps_the_flag() {
        let (title, desc) = resolve_title_and_description(
            Some(&"T".into()),
            None,
            Some(&"flag body".into()),
            "feat",
            "main",
        )
        .expect("resolve");
        assert_eq!(title, "T");
        assert_eq!(desc, "flag body");
    }

    #[test]
    fn title_with_both_descriptions_is_a_usage_error() {
        // Same contract as issue create: which description would win when both
        // sources are explicit is an implementation detail, so refuse.
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = desc_file(&tmp, "file body");
        let err = resolve_title_and_description(
            Some(&"T".into()),
            Some(&f),
            Some(&"flag body".into()),
            "feat",
            "main",
        )
        .expect_err("conflicting descriptions must fail");
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn both_descriptions_without_title_is_also_a_usage_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = desc_file(&tmp, "file body");
        let err = resolve_title_and_description(
            None,
            Some(&f),
            Some(&"flag body".into()),
            "feat",
            "main",
        )
        .expect_err("conflicting descriptions must fail");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn description_file_without_title_falls_back_to_branch_title() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = desc_file(&tmp, "body");
        let (title, desc) =
            resolve_title_and_description(None, Some(&f), None, "feat", "main").expect("resolve");
        assert_eq!(title, "feat into main");
        assert_eq!(desc, "body");
    }

    #[test]
    fn description_file_is_explicit_even_when_empty() {
        // An empty file means "no description", not "not provided": the fill
        // fallback in create() must not override it.
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = desc_file(&tmp, "");
        let (title, desc) =
            resolve_title_and_description(None, Some(&f), None, "feat", "main").expect("resolve");
        assert_eq!(title, "feat into main");
        assert_eq!(desc, "");
    }

    #[test]
    fn nothing_given_falls_back_to_branch_title_and_empty_description() {
        let (title, desc) =
            resolve_title_and_description(None, None, None, "feat", "main").expect("resolve");
        assert_eq!(title, "feat into main");
        assert_eq!(desc, "");
    }
}
