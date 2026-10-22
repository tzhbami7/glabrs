use serde_json::{json, Value};

use crate::cli::release_cmd::ReleaseArgs;
use crate::cli::Root;
use crate::cmd::helpers;
use crate::cmd::resolve;
use crate::error::Error;

pub fn run(root: &Root, args: &ReleaseArgs) -> Result<(), Error> {
    match args {
        ReleaseArgs::List {
            per_page,
            page,
            paginate,
        } => list(root, *per_page, *page, *paginate),
        ReleaseArgs::Create {
            tag,
            notes_file,
            name,
            ref_,
        } => create(root, tag, notes_file, name, ref_),
        ReleaseArgs::View { tag } => view(root, tag),
        ReleaseArgs::Delete { tag, yes } => delete(root, tag, *yes),
        ReleaseArgs::Upload {
            tag,
            target,
            name,
            link,
        } => upload(root, tag, target, name.as_deref(), *link),
    }
}

use helpers::project_path;

fn list(root: &Root, per_page: u32, page: Option<u32>, paginate: bool) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut query: Vec<(String, String)> = vec![("per_page".into(), per_page.to_string())];
    if let Some(p) = page {
        query.push(("page".into(), p.to_string()));
    }
    let path = format!("projects/{pp}/releases");
    let items: Vec<Value> = if paginate {
        ctx.client().get_paginated(&path, &query, None)?
    } else {
        ctx.client().get_json_vec(&path, &query)?
    };
    crate::render::table::render_list(
        &items,
        &["tag_name", "name", "released_at", "assets"],
        ctx.json,
    )?;
    Ok(())
}

fn create(
    root: &Root,
    tag: &str,
    notes_file: &Option<String>,
    name: &Option<String>,
    ref_: &Option<String>,
) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut body = json!({ "tag_name": tag });
    if let Some(f) = notes_file {
        body["description"] = Value::String(crate::render::pager::read_input(f)?);
    }
    if let Some(n) = name {
        body["name"] = json!(n);
    }
    if let Some(r) = ref_ {
        body["ref"] = json!(r);
    }
    let release: Value = ctx.client().send_json_value(
        "POST",
        &format!("projects/{pp}/releases"),
        &[],
        Some(body),
    )?;
    crate::render::table::render_one(
        &release,
        &["tag_name", "name", "description", "assets"],
        ctx.json,
    )?;
    Ok(())
}

fn view(root: &Root, tag: &str) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    let release: Value = ctx
        .client()
        .get_json_value(&format!("projects/{pp}/releases/{tag}"), &[])?;
    crate::render::table::render_one(
        &release,
        &["tag_name", "name", "description", "released_at", "assets"],
        ctx.json,
    )?;
    Ok(())
}

fn delete(root: &Root, tag: &str, yes: bool) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    if !yes {
        eprint!("Delete release {tag}? [y/N] ");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !line.trim().eq_ignore_ascii_case("y") {
            println!("aborted");
            return Ok(());
        }
    }
    ctx.client().send_json_value(
        "DELETE",
        &format!("projects/{pp}/releases/{tag}"),
        &[],
        None,
    )?;
    if !ctx.json {
        println!("Deleted release {tag}");
    }
    Ok(())
}

fn upload(
    root: &Root,
    tag: &str,
    target: &str,
    name: Option<&str>,
    link: bool,
) -> Result<(), Error> {
    let pp = project_path(root)?;
    let ctx = resolve(root, root.hostname.as_deref())?;

    if link || target.contains("://") {
        // Link asset: POST /releases/<tag>/assets/links
        let label = name.ok_or_else(|| Error::usage("--name required for link assets"))?;
        let body = json!({ "name": label, "url": target });
        let created: Value = ctx.client().send_json_value(
            "POST",
            &format!("projects/{pp}/releases/{tag}/assets/links"),
            &[],
            Some(body),
        )?;
        crate::render::table::render_one(&created, &["id", "name", "url"], ctx.json)?;
        return Ok(());
    }

    // File asset: upload to project, then link into the release.
    let path = std::path::Path::new(target);
    if !path.exists() {
        return Err(Error::not_found(format!("file not found: {target}")));
    }
    let file_name = name.map(String::from).unwrap_or_else(|| {
        path.file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    });
    let bytes = std::fs::read(path)?;
    let content_type = guess_content_type(&file_name);
    let uploaded: Value = upload_file(&ctx, &pp, &bytes, &content_type, &file_name)?;
    let url = uploaded
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::runtime("upload missing url"))?;
    // GitLab returns a MARKDOWN LINK for uploads: `[name](/uploads/<hash>/file)`.
    // Older instances returned a bare relative path. Extract the path, then
    // make it absolute against the instance ROOT (scheme + host + optional
    // prefix), not the project web URL — /uploads lives at instance scope.
    let rel = extract_upload_path(url)
        .ok_or_else(|| Error::runtime(format!("unexpected upload url: {url}")))?;
    let root_base = instance_root(&ctx.client().api_url);
    let full_url = format!("{root_base}{rel}");
    let body = json!({ "name": file_name, "url": full_url });
    let created: Value = ctx.client().send_json_value(
        "POST",
        &format!("projects/{pp}/releases/{tag}/assets/links"),
        &[],
        Some(body),
    )?;
    crate::render::table::render_one(&created, &["id", "name", "url"], ctx.json)?;
    Ok(())
}

fn upload_file(
    ctx: &crate::cmd::Ctx,
    pp: &str,
    bytes: &[u8],
    content_type: &str,
    file_name: &str,
) -> Result<Value, Error> {
    // Multipart upload to /uploads. The boundary must be unpredictable enough
    // not to collide with file content or concurrent uploads.
    let boundary = random_boundary();
    let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 512);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {content_type}\r\n\r\n",
            escape_header_value(file_name)
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let url = ctx.client().url(&format!("projects/{pp}/uploads"), &[]);
    let mut req = ctx.client().agent_post(&url)?;
    for (k, v) in ctx.client().extra_headers() {
        req = req.set(k, v);
    }
    let resp = req
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .send_bytes(&body)?;
    crate::api::client::decode_json(resp)
}

/// Random hex boundary per RFC 2046 (68 chars max; 32 hex chars is plenty).
fn random_boundary() -> String {
    let mut buf = [0u8; 16];
    getrandom(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn getrandom(buf: &mut [u8]) {
    use std::fs::File;
    use std::io::Read;
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .expect("read /dev/urandom");
}

/// Escape `"` and backslashes in a filename per RFC 2183 quoted-string rules.
fn escape_header_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Accept both a bare relative path (`/uploads/h/file`) and the markdown link
/// form (`[name](/uploads/h/file)`) that GitLab currently returns.
fn extract_upload_path(url: &str) -> Option<String> {
    if let Some(open) = url.find("](") {
        let rest = &url[open + 2..];
        let end = rest.find(')')?;
        return Some(rest[..end].to_string());
    }
    if url.starts_with('/') {
        Some(url.to_string())
    } else {
        None
    }
}

/// Reduce an API URL to the instance root: `https://host[:port]/prefix/api/v4`
/// -> `https://host[:port]/prefix`. `/uploads` lives at instance scope, and
/// the api_url (unlike a project web URL) ends in a known `/api/v4` suffix,
/// so the prefix boundary is unambiguous.
fn instance_root(api_url: &str) -> String {
    let trimmed = api_url.trim_end_matches('/');
    if let Some(base) = trimmed.strip_suffix("/api/v4") {
        if !base.is_empty() {
            return base.to_string();
        }
    }
    // No /api/v4 suffix: fall back to scheme + host[:port].
    match url::Url::parse(trimmed) {
        Ok(u) => {
            let host = match (u.host_str(), u.port()) {
                (Some(h), Some(p)) => format!("{h}:{p}"),
                (Some(h), None) => h.to_string(),
                _ => String::new(),
            };
            format!("{}://{}", u.scheme(), host)
        }
        Err(_) => trimmed.to_string(),
    }
}

fn guess_content_type(name: &str) -> String {
    match name.rsplit('.').next() {
        Some("gz") => "application/gzip".to_string(),
        Some("zip") => "application/zip".to_string(),
        Some("json") => "application/json".to_string(),
        Some("txt") => "text/plain".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_upload_path_forms() {
        assert_eq!(
            extract_upload_path("[foo.tar.gz](/uploads/abc123/foo.tar.gz)").unwrap(),
            "/uploads/abc123/foo.tar.gz"
        );
        assert_eq!(
            extract_upload_path("/uploads/abc123/foo.tar.gz").unwrap(),
            "/uploads/abc123/foo.tar.gz"
        );
        assert!(extract_upload_path("https://elsewhere/file").is_none());
    }

    #[test]
    fn instance_root_scenarios() {
        // Derived from the api_url, which ends in /api/v4.
        assert_eq!(
            instance_root("https://gitlab.corp.example/api/v4"),
            "https://gitlab.corp.example"
        );
        assert_eq!(
            instance_root("https://gitlab.corp.example/gitlab/api/v4"),
            "https://gitlab.corp.example/gitlab"
        );
        assert_eq!(
            instance_root("http://localhost:8080/api/v4"),
            "http://localhost:8080"
        );
    }

    #[test]
    fn random_boundaries_differ() {
        assert_ne!(random_boundary(), random_boundary());
        assert_eq!(random_boundary().len(), 32);
    }
}
