use std::fmt::Write as _;
use std::io::IsTerminal;

use serde_json::{json, Value};

use crate::cli::release_cmd::ReleaseArgs;
use crate::cli::Root;
use crate::cmd::helpers;
use crate::cmd::resolve;
use crate::error::Error;

/// Run one `release` subcommand.
///
/// # Errors
///
/// Returns [`Error`] when the request or a notes file read fails;
/// `delete` without `--yes` is a usage error in non-interactive sessions.
pub fn run(root: &Root, args: &ReleaseArgs) -> Result<(), Error> {
    match args {
        ReleaseArgs::List { list: list_opts } => list(root, list_opts),
        ReleaseArgs::Create {
            tag,
            notes_file,
            name,
            ref_,
        } => create(root, tag, notes_file.as_ref(), name.as_ref(), ref_.as_ref()),
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

fn list(root: &Root, opts: &crate::cli::ListOpts) -> Result<(), Error> {
    let per_page = opts.per_page;
    let page = opts.page;
    let paginate = opts.paginate;
    let pp = project_path(root, None)?;
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
        // `assets` is an object (renders as a useless `<object>`) and its
        // `sources`/`links` are arrays of objects; `assets.count` is the
        // readable projection. JSON mode still dumps the full response.
        &["tag_name", "name", "released_at", "assets.count"],
        ctx.json,
    )?;
    Ok(())
}

fn create(
    root: &Root,
    tag: &str,
    notes_file: Option<&String>,
    name: Option<&String>,
    ref_: Option<&String>,
) -> Result<(), Error> {
    let pp = project_path(root, None)?;
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
        Some(&body),
    )?;
    crate::render::table::render_one(
        &release,
        &["tag_name", "name", "description", "assets"],
        ctx.json,
    )?;
    Ok(())
}

fn view(root: &Root, tag: &str) -> Result<(), Error> {
    let pp = project_path(root, None)?;
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
    // Resolve the target before asking: a usage error (wrong host, no -R)
    // must surface before the user confirms anything.
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;
    if !yes {
        // Non-interactive stdin (CI, piped scripts) answers EOF to a prompt:
        // refusing up front keeps "aborted" from looking like success (exit 0)
        // to scripts keyed on the exit code.
        if !std::io::stdin().is_terminal() {
            return Err(Error::usage(
                "refusing to delete without --yes in a non-interactive session",
            ));
        }
        eprint!("Delete release {tag}? [y/N] ");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !line.trim().eq_ignore_ascii_case("y") {
            println!("aborted");
            return Err(Error::usage("aborted by user"));
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
    // NOTE: the file path below is fully in-memory (file bytes + the multipart
    // body are both buffered). Fine for typical release assets; a streaming
    // multipart body (ureq `send` with a `Read` + known length) is the fix if
    // multi-GB uploads ever matter.
    let pp = project_path(root, None)?;
    let ctx = resolve(root, root.hostname.as_deref())?;

    if link || target.contains("://") {
        // Link asset: POST /releases/<tag>/assets/links
        let label = name.ok_or_else(|| Error::usage("--name required for link assets"))?;
        let body = json!({ "name": label, "url": target });
        let created: Value = ctx.client().send_json_value(
            "POST",
            &format!("projects/{pp}/releases/{tag}/assets/links"),
            &[],
            Some(&body),
        )?;
        crate::render::table::render_one(&created, &["id", "name", "url"], ctx.json)?;
        return Ok(());
    }

    // File asset: upload to project, then link into the release.
    let path = std::path::Path::new(target);
    if !path.exists() {
        return Err(Error::not_found(format!("file not found: {target}")));
    }
    let file_name = name.map_or_else(
        || {
            path.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        },
        String::from,
    );
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
    // instance_root echoes unparseable api_urls unchanged; never paste that
    // into a link URL (same guard as web_base_from_api).
    let root_base = instance_root_link_base(&ctx.client().api_url)?;
    let full_url = format!("{root_base}{rel}");
    let body = json!({ "name": file_name, "url": full_url });
    let created: Value = ctx.client().send_json_value(
        "POST",
        &format!("projects/{pp}/releases/{tag}/assets/links"),
        &[],
        Some(&body),
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
    let boundary = random_boundary()?;
    let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 512);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {content_type}\r\n\r\n",
            escape_header_value(file_name)?
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let url = ctx.client().url(&format!("projects/{pp}/uploads"), &[]);
    // agent_post already applies auth (token + per-host headers); the retry
    // helper adds the same single 429 Retry-After as the JSON getters.
    let req = ctx.client().agent_post(&url)?;
    let resp = ctx.client().send_bytes_with_retry(
        req.set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        ),
        &body,
    )?;
    crate::api::client::decode_json(resp)
}

/// Random hex boundary per RFC 2046 (68 chars max; 32 hex chars is plenty).
fn random_boundary() -> Result<String, Error> {
    let mut buf = [0u8; 16];
    getrandom(&mut buf)?;
    let mut out = String::with_capacity(buf.len() * 2);
    for b in buf {
        let _ = write!(out, "{b:02x}");
    }
    Ok(out)
}

/// Fill `buf` with random bytes from /dev/urandom, propagating failure as a
/// runtime error (a panic here would abort an otherwise useful upload).
fn getrandom(buf: &mut [u8]) -> Result<(), Error> {
    use std::fs::File;
    use std::io::Read;
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .map_err(|e| Error::runtime(format!("read /dev/urandom: {e}")))
}

/// Escape a filename for a quoted-string header value per RFC 2183: quote and
/// backslash are escaped, and control characters are rejected outright so a
/// crafted filename cannot inject headers or break the multipart framing.
/// (CR/LF was the header-injection risk; the rest of C0 would corrupt the
/// boundary framing, which is just as fatal for the upload.)
///
/// # Errors
///
/// Returns [`Error::Usage`] when the filename contains a control character.
fn escape_header_value(s: &str) -> Result<String, Error> {
    if s.chars().any(char::is_control) {
        return Err(Error::usage(format!(
            "filename contains a control character (refusing to upload): {s:?}"
        )));
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    Ok(out)
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

fn guess_content_type(name: &str) -> String {
    match name.rsplit('.').next() {
        Some("gz") => "application/gzip".to_string(),
        Some("zip") => "application/zip".to_string(),
        Some("json") => "application/json".to_string(),
        Some("txt") => "text/plain".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

/// The instance root as a link-URL base: parsed absolute http(s) with a host
/// (`instance_root` echoes unparseable input unchanged), trailing slash
/// trimmed. The trim matters: `Url::to_string()` serializes a bare authority
/// with one (`https://h.example/`), so `format!("{base}{rel}")` with a
/// root-relative `/uploads/...` path produced `//uploads/...` link URLs for
/// non-prefixed instances.
///
/// # Errors
///
/// Returns [`Error::Runtime`] when the instance root cannot be derived.
fn instance_root_link_base(api_url: &str) -> Result<String, Error> {
    let root_base = crate::api::client::instance_root(api_url);
    url::Url::parse(&root_base)
        .ok()
        .filter(|u| u.host_str().is_some() && matches!(u.scheme(), "http" | "https"))
        .map(|u| u.to_string().trim_end_matches('/').to_string())
        .ok_or_else(|| Error::runtime(format!("cannot derive instance root from {api_url}")))
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
    fn escape_header_value_rejects_controls() {
        assert!(escape_header_value("ok.txt").is_ok());
        assert!(escape_header_value("bad\r\nHeader: x").is_err());
        assert!(escape_header_value("bad\nHeader: x").is_err());
        assert!(escape_header_value("bad\rHeader: x").is_err());
        // Other C0 controls would corrupt the multipart framing.
        assert!(escape_header_value("bad\tname.txt").is_err());
        assert!(escape_header_value("bad\0name.txt").is_err());
    }

    #[test]
    fn random_boundaries_differ() {
        let first = random_boundary().unwrap();
        let second = random_boundary().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.len(), 32);
    }

    #[test]
    fn instance_root_link_base_never_double_slashes() {
        // Regression: a bare-host instance root serializes with a trailing
        // slash (`Url::to_string()`), which made upload link URLs
        // `https://h.example//uploads/...` for non-prefixed instances.
        assert_eq!(
            instance_root_link_base("https://h.example/api/v4").unwrap(),
            "https://h.example"
        );
        // Prefixed instances keep their path and no slash is added.
        assert_eq!(
            instance_root_link_base("https://h.example/gitlab/api/v4").unwrap(),
            "https://h.example/gitlab"
        );
        // http (no-TLS instances keep their scheme) and custom ports.
        assert_eq!(
            instance_root_link_base("http://h.internal:8080/api/v4").unwrap(),
            "http://h.internal:8080"
        );
        // No /api/v4 suffix: scheme + host from the URL itself.
        assert_eq!(
            instance_root_link_base("https://h.example/custom").unwrap(),
            "https://h.example"
        );
        // Unparseable input is refused instead of becoming a garbage link.
        assert_eq!(
            instance_root_link_base("not a url")
                .unwrap_err()
                .exit_code(),
            1
        );
    }
}
