use std::io::Write as _;

use serde_json::{json, Value};

use crate::cli::ci_cmd::CiArgs;
use crate::cli::Root;
use crate::cmd::helpers;
use crate::cmd::resolve;
use crate::error::Error;

/// Generous stall bound for `trace --follow` without `--timeout`: consecutive
/// traces that 404'd while the job was already running. Once a job is running
/// its trace exists (even if empty), so this many missing traces in a row
/// means the runner is stuck or the trace endpoint unreachable. Queued jobs
/// (created/pending) legitimately produce no trace and are not bounded.
const MAX_NOT_YET_RUNNING_POLLS: usize = 20;

/// Run one `ci` subcommand.
///
/// # Errors
///
/// Returns [`Error`] when the request fails; `status --wait` and
/// `trace --follow` mirror the pipeline/job exit status.
pub fn run(root: &Root, args: &CiArgs) -> Result<(), Error> {
    match args {
        CiArgs::List {
            ref_,
            status,
            list: list_opts,
        } => list(root, ref_.as_deref(), status.as_deref(), list_opts),
        CiArgs::Status {
            ref_,
            wait,
            timeout,
        } => status(root, ref_.as_deref(), *wait, *timeout),
        CiArgs::View { pipeline_id } => view(root, *pipeline_id),
        CiArgs::Trace {
            target,
            follow,
            timeout,
            pipeline,
        } => trace(root, target, *follow, *timeout, *pipeline),
        CiArgs::Run { ref_, variables } => run_pipeline(root, ref_, variables),
        CiArgs::Retry { pipeline_id } => retry(root, *pipeline_id),
        CiArgs::Cancel { pipeline_id } => cancel(root, *pipeline_id),
        CiArgs::Lint { file } => lint(root, file.as_deref()),
    }
}

use helpers::project_path;

fn list(
    root: &Root,
    ref_: Option<&str>,
    status: Option<&str>,
    opts: &crate::cli::ListOpts,
) -> Result<(), Error> {
    let per_page = opts.per_page;
    let page = opts.page;
    let paginate = opts.paginate;
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut query: Vec<(String, String)> = vec![("per_page".into(), per_page.to_string())];
    if let Some(r) = ref_ {
        query.push(("ref".into(), r.to_string()));
    }
    if let Some(s) = status {
        query.push(("status".into(), s.to_string()));
    }
    // Pushed unconditionally: honored as the start page under --paginate
    // (get_paginated dedups it) and as the single page otherwise.
    if let Some(p) = page {
        query.push(("page".into(), p.to_string()));
    }
    let path = format!("projects/{pp}/pipelines");
    let items: Vec<Value> = if paginate {
        ctx.client().get_paginated(&path, &query, None)?
    } else {
        ctx.client().get_json_vec(&path, &query)?
    };
    crate::render::table::render_list(
        &items,
        &["id", "iid", "ref", "status", "created_at", "updated_at"],
        ctx.json,
    )?;
    Ok(())
}

fn status(root: &Root, ref_: Option<&str>, wait: bool, timeout: Option<u64>) -> Result<(), Error> {
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut query: Vec<(String, String)> = Vec::new();
    if let Some(r) = ref_ {
        query.push(("ref".into(), r.to_string()));
    }
    // Explicit latest-first ordering instead of relying on GitLab's default
    // list sort: `trace` (find_job) uses order_by=id&sort=desc, and both
    // commands must agree on which pipeline "latest" names.
    query.push(("order_by".into(), "id".into()));
    query.push(("sort".into(), "desc".into()));
    let mut pipeline: Value = ctx
        .client()
        .get_json_value(&format!("projects/{pp}/pipelines"), &query)?
        .as_array()
        .and_then(|a| a.first().cloned())
        .ok_or_else(|| Error::not_found("no pipelines found"))?;

    let poll_interval = std::time::Duration::from_secs(3);
    let deadline = timeout.map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s));
    loop {
        let st = pipeline
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if !wait || is_terminal_state(st) {
            if ctx.json {
                crate::render::table::print_json(&pipeline)?;
            } else {
                let id = pipeline
                    .get("id")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                println!("pipeline #{id}: {st}");
            }
            return pipeline_exit(st, wait);
        }
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            return Err(Error::runtime(format!(
                "timed out waiting for pipeline (status: {st}); use a larger --timeout or poll without --wait"
            )));
        }
        std::thread::sleep(poll_interval);
        let id = pipeline
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        pipeline = ctx
            .client()
            .get_json_value(&format!("projects/{pp}/pipelines/{id}"), &[])?;
    }
}

fn view(root: &Root, pipeline_id: u64) -> Result<(), Error> {
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let pipeline: Value = ctx
        .client()
        .get_json_value(&format!("projects/{pp}/pipelines/{pipeline_id}"), &[])?;
    crate::render::table::render_one(
        &pipeline,
        &["id", "iid", "ref", "status", "sha", "web_url", "created_at"],
        ctx.json,
    )?;
    Ok(())
}

fn trace(
    root: &Root,
    target: &str,
    follow: bool,
    timeout: Option<u64>,
    pipeline: Option<u64>,
) -> Result<(), Error> {
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let job = match pipeline {
        Some(pid) => find_job_in_pipeline(&ctx, &pp, pid, target)?,
        None => find_job(&ctx, &pp, target)?,
    };
    let job_id = job
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    // Bytes fetched so far: the offset into the next trace body. A shorter
    // body than the last poll (GitLab can rewrite/shrink traces) is treated as
    // a reset — see `next_trace_chunk`.
    let mut printed = 0usize;
    // Bytes fetched but not yet printed: a multi-byte UTF-8 char straddling a
    // poll boundary is held here until complete instead of rendering as U+FFFD.
    let mut carry: Vec<u8> = Vec::new();
    // Consecutive missing traces while the job was running (see the check
    // below); reset as soon as a trace arrives or the status leaves running.
    let mut not_yet_running_polls = 0usize;
    let deadline = timeout.map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s));
    loop {
        // Fetch the raw trace.
        let url = format!("projects/{pp}/jobs/{job_id}/trace");
        let bytes = fetch_raw(&ctx, &url)?;
        if let Some(b) = bytes.as_deref() {
            if let Some(chunk) = next_trace_chunk(b, &mut printed) {
                carry.extend_from_slice(chunk);
                let text = take_trace_text(&mut carry);
                if !text.is_empty() {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
            }
        }
        let job: Value = ctx
            .client()
            .get_json_value(&format!("projects/{pp}/jobs/{job_id}"), &[])?;
        let st = job
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if !follow || is_terminal_state(st) {
            // Flush a truncated UTF-8 tail held in carry (the log ended
            // mid-character): nothing stays unprinted.
            if !carry.is_empty() {
                print!("{}", String::from_utf8_lossy(&carry));
                let _ = std::io::stdout().flush();
            }
            if !ctx.json {
                eprintln!();
                if is_terminal_state(st) {
                    eprintln!("job #{job_id} finished: {st}");
                } else {
                    eprintln!("job #{job_id} is still {st} (use --follow to keep streaming)");
                }
            }
            return job_exit(st, follow);
        }
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            return Err(Error::runtime(format!(
                "timed out following job #{job_id} (status: {st}); use a larger --timeout or run without --follow"
            )));
        }
        // Generous stall bound for the no-timeout case (see the constant).
        if advance_stall_counter(&mut not_yet_running_polls, bytes.as_deref(), st) {
            return Err(Error::runtime(format!(
                "no trace after {not_yet_running_polls} polls while job #{job_id} is running (the runner may be stuck or the trace endpoint unreachable); use --timeout to bound this or ci trace without --follow to print what is there"
            )));
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

/// The new bytes of a trace body: the suffix past `printed` (the previous
/// body's length), or all of it when the body shrank (GitLab can rewrite
/// traces, e.g. on archive — resuming from the stale offset would silently
/// skip content until it grew past it again). `None` when nothing is new.
/// Updates `printed` to the new body length.
///
/// A shrink restarts from the beginning: for the reset-and-regrow case the
/// regrown content is genuinely new and must print; the cost is a re-print of
/// any prefix the shrunken body still shares with the old one, which is the
/// safer failure mode (never silently loses data).
///
/// Pure so the offset/reset behavior is testable without a server.
fn next_trace_chunk<'a>(body: &'a [u8], printed: &mut usize) -> Option<&'a [u8]> {
    match body.len().cmp(printed) {
        std::cmp::Ordering::Greater => {
            let chunk = &body[*printed..];
            *printed = body.len();
            Some(chunk)
        }
        // Shrunk: restart from the beginning so nothing is skipped.
        std::cmp::Ordering::Less => {
            *printed = body.len();
            Some(body)
        }
        std::cmp::Ordering::Equal => None,
    }
}

/// Advance the trace-stall counter: a missing trace while the job is already
/// running counts up; anything else (a trace arrived, or the status left
/// running) resets it. Reports when the stall bound is hit.
///
/// Pure so the reset/increment/threshold behavior is testable without a
/// server.
fn advance_stall_counter(counter: &mut usize, bytes: Option<&[u8]>, st: &str) -> bool {
    if bytes.is_none() && st == "running" {
        *counter += 1;
    } else {
        *counter = 0;
    }
    *counter >= MAX_NOT_YET_RUNNING_POLLS
}

/// Extract printable text from the carry buffer, handling all three UTF-8
/// cases:
///
/// - fully valid: everything prints, buffer empties;
/// - incomplete trailing sequence (a char straddles the poll boundary):
///   the valid prefix prints, the tail is kept for the next poll;
/// - genuinely invalid bytes (`error_len() == Some(n)`, e.g. a binary byte
///   in a build log): they render as U+FFFD and are drained, so the stream
///   keeps flowing instead of blocking on them forever.
fn take_trace_text(carry: &mut Vec<u8>) -> String {
    match std::str::from_utf8(carry) {
        Ok(text) => {
            let out = text.to_string();
            carry.clear();
            out
        }
        Err(e) => {
            let valid = e.valid_up_to();
            let mut out = String::from_utf8_lossy(&carry[..valid]).into_owned();
            match e.error_len() {
                // Trailing bytes are an incomplete char: keep them for the
                // next poll.
                None => {
                    carry.drain(..valid);
                }
                // Invalid sequence of `n` bytes: emit one U+FFFD (the same
                // replacement `from_utf8_lossy` uses) and move past it.
                Some(n) => {
                    out.push('\u{FFFD}');
                    carry.drain(..valid + n);
                }
            }
            out
        }
    }
}

/// The raw trace, or `None` while the endpoint 404s (no trace produced yet).
fn fetch_raw(ctx: &crate::cmd::Ctx, path: &str) -> Result<Option<Vec<u8>>, Error> {
    let url = ctx.client().url(path, &[]);
    // GitLab 404s the trace of jobs that have not produced one yet
    // (created/pending). During a follow that is "no output yet", not an
    // error: keep polling until the first chunk appears.
    match ctx.client().get_bytes(&url) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(Error::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

fn find_job(ctx: &crate::cmd::Ctx, pp: &str, target: &str) -> Result<Value, Error> {
    if let Ok(id) = target.parse::<u64>() {
        return ctx
            .client()
            .get_json_value(&format!("projects/{pp}/jobs/{id}"), &[]);
    }
    // Look up by name in the most recently created pipeline (any branch).
    // Fetching every job of the project and returning the first name match
    // would pick an arbitrary old pipeline's job on any active project.
    let pipelines: Vec<Value> = ctx.client().get_json_vec(
        &format!("projects/{pp}/pipelines"),
        &[
            ("per_page".into(), "1".into()),
            ("order_by".into(), "id".into()),
            ("sort".into(), "desc".into()),
        ],
    )?;
    let pipeline_id = pipelines
        .first()
        .and_then(|p| p.get("id").and_then(serde_json::Value::as_u64))
        .ok_or_else(|| Error::not_found("no pipelines found"))?;
    // Paginated: large pipelines (matrix jobs) can exceed the first 100.
    let jobs: Vec<Value> = ctx.client().get_paginated(
        &format!("projects/{pp}/pipelines/{pipeline_id}/jobs"),
        &[],
        None,
    )?;
    for j in jobs {
        if j.get("name").and_then(|v| v.as_str()) == Some(target) {
            return Ok(j);
        }
    }
    Err(Error::not_found(format!(
        "job {target:?} not found in latest pipeline #{pipeline_id}"
    )))
}

/// Look up a job by ID or name within a specific pipeline (the `--pipeline`
/// escape hatch for jobs outside the latest pipeline's first 100).
fn find_job_in_pipeline(
    ctx: &crate::cmd::Ctx,
    pp: &str,
    pipeline_id: u64,
    target: &str,
) -> Result<Value, Error> {
    // Numeric IDs are matched within the pipeline, not passed straight to the
    // project-level endpoint: `--pipeline 999` must not trace a job that
    // belongs to a different pipeline.
    let wanted = target.parse::<u64>().ok();
    let jobs: Vec<Value> = ctx.client().get_paginated(
        &format!("projects/{pp}/pipelines/{pipeline_id}/jobs"),
        &[],
        None,
    )?;
    for j in jobs {
        let matches = match wanted {
            Some(id) => j.get("id").and_then(serde_json::Value::as_u64) == Some(id),
            None => j.get("name").and_then(|v| v.as_str()) == Some(target),
        };
        if matches {
            return Ok(j);
        }
    }
    Err(Error::not_found(match wanted {
        Some(id) => format!("job #{id} not found in pipeline #{pipeline_id}"),
        None => format!("job {target:?} not found in pipeline #{pipeline_id}"),
    }))
}

fn run_pipeline(root: &Root, ref_: &str, variables: &[String]) -> Result<(), Error> {
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut vars = Vec::new();
    for v in variables {
        match v.split_once('=') {
            Some((k, val)) => vars.push(json!({
                "key": k,
                "value": val,
            })),
            None => {
                return Err(Error::usage(format!(
                    "bad --variable {v:?} (want key=value)"
                )))
            }
        }
    }
    let mut body = json!({ "ref": ref_ });
    if !vars.is_empty() {
        body["variables"] = Value::Array(vars);
    }
    let pipeline: Value = ctx.client().send_json_value(
        "POST",
        &format!("projects/{pp}/pipeline"),
        &[],
        Some(&body),
    )?;
    crate::render::table::render_one(
        &pipeline,
        &["id", "iid", "ref", "status", "web_url"],
        ctx.json,
    )?;
    Ok(())
}

fn retry(root: &Root, pipeline_id: u64) -> Result<(), Error> {
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let pipeline: Value = ctx.client().send_json_value(
        "POST",
        &format!("projects/{pp}/pipelines/{pipeline_id}/retry"),
        &[],
        None,
    )?;
    crate::render::table::render_one(&pipeline, &["id", "ref", "status"], ctx.json)?;
    Ok(())
}

fn cancel(root: &Root, pipeline_id: u64) -> Result<(), Error> {
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let pipeline: Value = ctx.client().send_json_value(
        "POST",
        &format!("projects/{pp}/pipelines/{pipeline_id}/cancel"),
        &[],
        None,
    )?;
    crate::render::table::render_one(&pipeline, &["id", "ref", "status"], ctx.json)?;
    Ok(())
}

fn lint(root: &Root, file: Option<&str>) -> Result<(), Error> {
    let ctx = resolve(root, root.hostname.as_deref())?;
    let content = match file {
        Some(p) => crate::render::pager::read_input(p)?,
        None => std::fs::read_to_string(".gitlab-ci.yml")
            .map_err(|e| Error::runtime(format!(".gitlab-ci.yml: {e}")))?,
    };
    // Project context is optional for lint but the API needs a project; use
    // detected project when available, else CI_LINT endpoint without project.
    let pp = project_path(root, None).ok();
    let body = json!({ "content": content });
    let resp: Value = match pp {
        Some(pp) => ctx.client().send_json_value(
            "POST",
            &format!("projects/{pp}/ci/lint"),
            &[],
            Some(&body),
        ),
        None => ctx
            .client()
            .send_json_value("POST", "ci/lint", &[], Some(&body)),
    }?;
    crate::render::table::render_one(&resp, &["valid", "errors", "warnings"], ctx.json)?;
    Ok(())
}

fn is_terminal_state(st: &str) -> bool {
    matches!(st, "success" | "failed" | "canceled" | "skipped")
}

fn pipeline_exit(st: &str, waited: bool) -> Result<(), Error> {
    // Exhaustive on the documented status set; anything unknown fails so a
    // new GitLab status string cannot mask a still-running pipeline as
    // success in scripts. `skipped` is a success like in glab: a pipeline
    // that was skipped by rules did not fail.
    //
    // Without `--wait` the command is a snapshot, not a gate: reporting
    // `running`/`pending` succeeds (exit 0) so scripts can poll without
    // treating "not done yet" as a failure; `--wait` still fails when it
    // would otherwise block forever on a non-terminal state.
    match st {
        "success" | "skipped" => Ok(()),
        "failed" => Err(Error::runtime("pipeline failed")),
        "canceled" => Err(Error::runtime("pipeline canceled")),
        "running" | "pending" | "created" | "waiting_for_resource" | "preparing" if !waited => {
            Ok(())
        }
        other => Err(Error::runtime(format!(
            "pipeline not finished (status: {other})"
        ))),
    }
}

fn job_exit(st: &str, followed: bool) -> Result<(), Error> {
    // Same contract as [`pipeline_exit`], including skipped-as-success:
    // a `trace` without `--follow` is a snapshot, so a still-active job
    // exits 0 (with the "still <status>" notice) instead of failing.
    match st {
        "success" | "skipped" => Ok(()),
        "failed" => Err(Error::runtime("job failed")),
        "canceled" => Err(Error::runtime("job canceled")),
        "running" | "pending" | "created" | "waiting_for_resource" | "preparing" if !followed => {
            Ok(())
        }
        other => Err(Error::runtime(format!(
            "job not finished (status: {other})"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_exit_codes() {
        assert!(pipeline_exit("success", true).is_ok());
        // Like glab: rules skipped the pipeline, it did not fail.
        assert!(pipeline_exit("skipped", true).is_ok());
        assert!(pipeline_exit("failed", true).is_err());
        assert!(pipeline_exit("canceled", true).is_err());
        // With --wait a non-terminal status must not read as success.
        assert!(pipeline_exit("running", true).is_err());
        assert!(pipeline_exit("pending", true).is_err());
        assert!(pipeline_exit("some_future_status", true).is_err());
    }

    #[test]
    fn pipeline_snapshot_exit_codes() {
        // Without --wait the command reports the current state: still-running
        // statuses exit 0 (scripts may poll), terminal failures still fail,
        // and an unknown status never reads as success.
        for st in [
            "running",
            "pending",
            "created",
            "waiting_for_resource",
            "preparing",
        ] {
            assert!(
                pipeline_exit(st, false).is_ok(),
                "{st} must be a snapshot ok"
            );
        }
        assert!(pipeline_exit("failed", false).is_err());
        assert!(pipeline_exit("canceled", false).is_err());
        assert!(pipeline_exit("some_future_status", false).is_err());
    }

    #[test]
    fn job_exit_codes() {
        assert!(job_exit("success", true).is_ok());
        assert!(job_exit("skipped", true).is_ok()); // same contract as pipelines
        assert!(job_exit("failed", true).is_err());
        assert!(job_exit("running", true).is_err());
    }

    #[test]
    fn job_snapshot_exit_codes() {
        // trace without --follow is a snapshot: active statuses exit 0, the
        // notice (not "finished") is printed, terminal failures still fail.
        for st in [
            "running",
            "pending",
            "created",
            "waiting_for_resource",
            "preparing",
        ] {
            assert!(job_exit(st, false).is_ok(), "{st} must be a snapshot ok");
        }
        assert!(job_exit("failed", false).is_err());
        assert!(job_exit("canceled", false).is_err());
        assert!(job_exit("some_future_status", false).is_err());
    }

    #[test]
    fn trace_text_valid_and_incomplete() {
        // Fully valid input prints and empties the buffer.
        let mut carry = b"hello".to_vec();
        assert_eq!(take_trace_text(&mut carry), "hello");
        assert_eq!(carry, b"");

        // A multi-byte char straddling a poll boundary is held until complete:
        // valid prefix prints, the tail is kept.
        let mut carry = "héllo".as_bytes().to_vec();
        carry.truncate(2); // 'é' is 2 bytes; only its first byte arrives
        let out = take_trace_text(&mut carry);
        assert_eq!(out, "h");
        assert_eq!(carry, &"é".as_bytes()[..1]); // incomplete é kept
                                                 // Next poll completes the char.
        carry.push(b'\xA9'); // é's continuation byte
        assert_eq!(take_trace_text(&mut carry), "é");
        assert_eq!(carry, b"");
        // The rest of the log follows.
        carry.extend_from_slice(b"llo");
        assert_eq!(take_trace_text(&mut carry), "llo");
        assert_eq!(carry, b"");
    }

    #[test]
    fn trace_text_invalid_byte_keeps_stream_flowing() {
        // A genuinely invalid byte renders as U+FFFD and is drained, so the
        // rest of the log still prints (the old code blocked on it forever).
        let mut carry = b"ok\xFFmore".to_vec();
        assert_eq!(take_trace_text(&mut carry), "ok\u{FFFD}");
        assert_eq!(carry, b"more");
        assert_eq!(take_trace_text(&mut carry), "more");
        assert_eq!(carry, b"");

        // Invalid bytes at the very start (valid_up_to() == 0).
        let mut carry = b"\xFFok".to_vec();
        assert_eq!(take_trace_text(&mut carry), "\u{FFFD}");
        assert_eq!(carry, b"ok");
    }

    #[test]
    fn trace_text_split_invalid_byte() {
        // An invalid byte split across polls still drains correctly.
        let mut carry = b"a\xC3".to_vec(); // incomplete 2-byte start...
        assert_eq!(take_trace_text(&mut carry), "a");
        assert_eq!(carry, b"\xC3");
        carry.push(b'\x28'); // ...completed with an invalid continuation
        assert_eq!(take_trace_text(&mut carry), "\u{FFFD}");
        // Only the invalid byte is drained; `(` is valid and prints next.
        assert_eq!(carry, b"(");
        assert_eq!(take_trace_text(&mut carry), "(");
        assert_eq!(carry, b"");
    }

    #[test]
    fn next_trace_chunk_handles_growth_and_shrink() {
        // Growth: only the suffix past the previous length is new.
        let mut printed = 0;
        assert_eq!(
            next_trace_chunk(b"hello", &mut printed),
            Some(&b"hello"[..])
        );
        assert_eq!(printed, 5);
        assert_eq!(
            next_trace_chunk(b"hello world", &mut printed),
            Some(&b" world"[..])
        );
        assert_eq!(printed, 11);
        // Unchanged body: nothing new.
        assert_eq!(next_trace_chunk(b"hello world", &mut printed), None);
        assert_eq!(printed, 11);
        // Shrunk body (GitLab can rewrite traces): restart from the beginning
        // instead of silently skipping content.
        assert_eq!(next_trace_chunk(b"abc", &mut printed), Some(&b"abc"[..]));
        assert_eq!(printed, 3);
        // Empty body after content: same shrink rule.
        assert_eq!(next_trace_chunk(b"", &mut printed), Some(&b""[..]));
        assert_eq!(printed, 0);
        // Regrowth after the empty body: the new content prints.
        assert_eq!(next_trace_chunk(b"new", &mut printed), Some(&b"new"[..]));
        assert_eq!(printed, 3);
    }

    #[test]
    fn stall_counter_counts_missing_trace_only_while_running() {
        // A missing trace while the job is already running counts up...
        let mut counter = 0;
        assert!(!advance_stall_counter(&mut counter, None, "running"));
        assert_eq!(counter, 1);
        // ...anything else resets it: a trace arrived, or the status left
        // running (queued jobs legitimately produce no trace).
        assert!(!advance_stall_counter(
            &mut counter,
            Some(b"out"),
            "running"
        ));
        assert_eq!(counter, 0);
        assert!(!advance_stall_counter(
            &mut counter,
            Some(b"out"),
            "running"
        ));
        assert_eq!(counter, 0);
        assert!(!advance_stall_counter(&mut counter, None, "running"));
        assert_eq!(counter, 1);
        assert!(!advance_stall_counter(&mut counter, None, "pending"));
        assert_eq!(counter, 0);
        // Empty trace (exists, zero bytes) does not count either.
        assert!(!advance_stall_counter(&mut counter, Some(b""), "running"));
        assert_eq!(counter, 0);
        // The threshold itself reports.
        let mut counter = MAX_NOT_YET_RUNNING_POLLS - 1;
        assert!(advance_stall_counter(&mut counter, None, "running"));
        assert_eq!(counter, MAX_NOT_YET_RUNNING_POLLS);
    }
}
