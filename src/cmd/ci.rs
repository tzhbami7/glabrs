use serde_json::{json, Value};

use crate::cli::ci_cmd::CiArgs;
use crate::cli::Root;
use crate::cmd::resolve;
use crate::error::Error;

pub fn run(root: &Root, args: &CiArgs) -> Result<(), Error> {
    match args {
        CiArgs::List {
            ref_,
            status,
            per_page,
            page,
            paginate,
        } => list(root, ref_, status, *per_page, *page, *paginate),
        CiArgs::Status { ref_, wait } => status(root, ref_, *wait),
        CiArgs::View { pipeline_id } => view(root, *pipeline_id),
        CiArgs::Trace { target, follow } => trace(root, target, *follow),
        CiArgs::Run { ref_, variables } => run_pipeline(root, ref_, variables),
        CiArgs::Retry { pipeline_id } => retry(root, *pipeline_id),
        CiArgs::Cancel { pipeline_id } => cancel(root, *pipeline_id),
        CiArgs::Lint { file } => lint(root, file),
    }
}

fn project_path(root: &Root) -> Result<String, Error> {
    if let Some(r) = &root.repo {
        return Ok(r.trim_matches('/').replace('/', "%2F"));
    }
    Ok(crate::git::detect_remote()?
        .project_path
        .replace('/', "%2F"))
}

fn list(
    root: &Root,
    ref_: &Option<String>,
    status: &Option<String>,
    per_page: u32,
    page: Option<u32>,
    paginate: bool,
) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut query: Vec<(String, String)> = vec![("per_page".into(), per_page.to_string())];
    if let Some(r) = ref_ {
        query.push(("ref".into(), r.clone()));
    }
    if let Some(s) = status {
        query.push(("status".into(), s.clone()));
    }
    let path = format!("projects/{pp}/pipelines");
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
        &["id", "iid", "ref", "status", "created_at", "updated_at"],
        ctx.json,
    )?;
    Ok(())
}

fn status(root: &Root, ref_: &Option<String>, wait: bool) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut query: Vec<(String, String)> = Vec::new();
    if let Some(r) = ref_ {
        query.push(("ref".into(), r.clone()));
    }
    let mut pipeline: Value = ctx
        .client()
        .get_json_value(&format!("projects/{pp}/pipelines"), &query)?
        .as_array()
        .and_then(|a| a.first().cloned())
        .ok_or_else(|| Error::not_found("no pipelines found"))?;

    let poll_interval = std::time::Duration::from_secs(3);
    loop {
        let st = pipeline
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if !wait || is_terminal_state(st) {
            if ctx.json {
                crate::render::table::print_json(&pipeline)?;
            } else {
                let id = pipeline.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                println!("pipeline #{}: {}", id, st);
            }
            return pipeline_exit(st);
        }
        std::thread::sleep(poll_interval);
        let id = pipeline.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
        pipeline = ctx
            .client()
            .get_json_value(&format!("projects/{pp}/pipelines/{id}"), &[])?;
    }
}

fn view(root: &Root, pipeline_id: u64) -> Result<(), Error> {
    let pp = project_path(root)?;
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

fn trace(root: &Root, target: &str, follow: bool) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let job = find_job(&ctx, &pp, target)?;
    let job_id = job.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
    let mut printed = 0usize;
    loop {
        // Fetch the raw trace.
        let url = format!("projects/{pp}/jobs/{job_id}/trace");
        let bytes = fetch_raw(&ctx, &url)?;
        if bytes.len() > printed {
            let chunk = String::from_utf8_lossy(&bytes[printed..]);
            print!("{chunk}");
            printed = bytes.len();
        }
        let job: Value = ctx
            .client()
            .get_json_value(&format!("projects/{pp}/jobs/{job_id}"), &[])?;
        let st = job
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if !follow || is_terminal_state(st) {
            if !ctx.json {
                eprintln!();
                eprintln!("job #{job_id} finished: {st}");
            }
            return job_exit(st);
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

fn fetch_raw(ctx: &crate::cmd::Ctx, path: &str) -> Result<Vec<u8>, Error> {
    let url = ctx.client().url(path, &[]);
    let resp = ctx.client().agent_get(&url)?.call()?;
    let mut buf = Vec::new();
    use std::io::Read;
    resp.into_reader().read_to_end(&mut buf)?;
    Ok(buf)
}

fn find_job(ctx: &crate::cmd::Ctx, pp: &str, target: &str) -> Result<Value, Error> {
    if let Ok(id) = target.parse::<u64>() {
        return ctx
            .client()
            .get_json_value(&format!("projects/{pp}/jobs/{id}"), &[]);
    }
    // Look up by name in the latest pipeline for the default branch. Fetching
    // every job of the project and returning the first name match would pick
    // an arbitrary old pipeline's job on any active project.
    let pipelines: Vec<Value> = ctx
        .client()
        .get_json_vec(
            &format!("projects/{pp}/pipelines"),
            &[("per_page".into(), "1".into()), ("order_by".into(), "id".into()), ("sort".into(), "desc".into())],
        )?;
    let pipeline_id = pipelines
        .first()
        .and_then(|p| p.get("id").and_then(|v| v.as_u64()))
        .ok_or_else(|| Error::not_found("no pipelines found"))?;
    let jobs: Vec<Value> = ctx.client().get_json_vec(
        &format!("projects/{pp}/pipelines/{pipeline_id}/jobs"),
        &[("per_page".into(), "100".into())],
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

fn run_pipeline(root: &Root, ref_: &str, variables: &[String]) -> Result<(), Error> {
    let pp = project_path(root)?;
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
        Some(body),
    )?;
    crate::render::table::render_one(
        &pipeline,
        &["id", "iid", "ref", "status", "web_url"],
        ctx.json,
    )?;
    Ok(())
}

fn retry(root: &Root, pipeline_id: u64) -> Result<(), Error> {
    let pp = project_path(root)?;
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
    let pp = project_path(root)?;
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

fn lint(root: &Root, file: &Option<String>) -> Result<(), Error> {
    let ctx = resolve(root, root.hostname.as_deref())?;
    let content = match file {
        Some(p) => crate::render::pager::read_input(p)?,
        None => std::fs::read_to_string(".gitlab-ci.yml")
            .map_err(|e| Error::runtime(format!(".gitlab-ci.yml: {e}")))?,
    };
    // Project context is optional for lint but the API needs a project; use
    // detected project when available, else CI_LINT endpoint without project.
    let pp = project_path(root).ok();
    let body = json!({ "content": content });
    let resp: Value = match pp {
        Some(pp) => {
            ctx.client()
                .send_json_value("POST", &format!("projects/{pp}/ci/lint"), &[], Some(body))
        }
        None => ctx
            .client()
            .send_json_value("POST", "ci/lint", &[], Some(body)),
    }?;
    crate::render::table::render_one(&resp, &["valid", "errors", "warnings"], ctx.json)?;
    Ok(())
}

fn is_terminal_state(st: &str) -> bool {
    matches!(st, "success" | "failed" | "canceled" | "skipped")
}

fn pipeline_exit(st: &str) -> Result<(), Error> {
    // Exhaustive on the documented status set; anything unknown fails so a
    // new GitLab status string cannot mask a still-running pipeline as
    // success in scripts.
    match st {
        "success" => Ok(()),
        "failed" => Err(Error::runtime("pipeline failed")),
        "canceled" => Err(Error::runtime("pipeline canceled")),
        "skipped" => Err(Error::runtime("pipeline skipped")),
        other => Err(Error::runtime(format!(
            "pipeline not finished (status: {other})"
        ))),
    }
}

fn job_exit(st: &str) -> Result<(), Error> {
    match st {
        "success" => Ok(()),
        "failed" => Err(Error::runtime("job failed")),
        "canceled" => Err(Error::runtime("job canceled")),
        "skipped" => Err(Error::runtime("job skipped")),
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
        assert!(pipeline_exit("success").is_ok());
        assert!(pipeline_exit("failed").is_err());
        assert!(pipeline_exit("canceled").is_err());
        assert!(pipeline_exit("skipped").is_err());
        // Unknown/running statuses must not read as success.
        assert!(pipeline_exit("running").is_err());
        assert!(pipeline_exit("pending").is_err());
        assert!(pipeline_exit("some_future_status").is_err());
    }

    #[test]
    fn job_exit_codes() {
        assert!(job_exit("success").is_ok());
        assert!(job_exit("failed").is_err());
        assert!(job_exit("running").is_err());
    }
}
