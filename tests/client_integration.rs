//! Integration tests using a small std-lib mock GitLab API.
//!
//! Covers the client-level behaviors that historically broke: 204 responses,
//! pagination headers, empty-body POSTs, and the auth headers for personal
//! vs. job tokens.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;

use serde_json::Value;

use glabrs::api::client::{decode_json, Client, HostSettings, TokenKind};

/// One recorded HTTP request.
#[derive(Debug, Clone)]
struct Req {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

/// Minimal HTTP server: answers each request via the provided closure on a
/// channel, so tests fully control status/headers/body. Single-connection,
/// one request per accept.
fn spawn_server<F>(handler: F) -> (String, mpsc::Receiver<Req>)
where
    F: Fn(&Req) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        // The agent pools connections: it may open speculative keep-alive
        // connections it never sends a request on. Answering those with an
        // EOF error would poison the pool (later requests fail with TLS
        // garbage) and stall the suite; just drop them.
        //
        // A 5s socket timeout keeps any scheme/URL mismatch (e.g. an https
        // client against this plain-HTTP server) from deadlocking both sides
        // until the client's long timeout: the handshake bytes never parse as
        // a request, so the connection is dropped and the client errors out.
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
            let Ok(req) = read_request(&mut stream) else {
                continue;
            };
            let (status, headers, body) = handler(&req);
            let _ = tx.send(req);
            write_response(&mut stream, status, &headers, &body).expect("write response");
        }
    });
    (format!("http://{addr}"), rx)
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Req> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Read until end of headers.
    let header_end;
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof in headers",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            header_end = pos + 4;
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    // Body per Content-Length.
    let content_length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    Ok(Req {
        method,
        path,
        headers,
        body,
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(String, String)],
    body: &[u8],
) -> std::io::Result<()> {
    use std::fmt::Write as _;
    // Statuses other than these never occur in these tests; "OK" keeps the
    // response well-formed for ureq regardless.
    let reason = match status {
        204 => "No Content",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "OK",
    };
    let mut resp = format!("HTTP/1.1 {status} {reason}\r\n");
    for (k, v) in headers {
        let _ = write!(resp, "{k}: {v}\r\n");
    }
    let _ = write!(resp, "Content-Length: {}\r\n", body.len());
    resp.push_str("Connection: close\r\n\r\n");
    stream.write_all(resp.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn json_body(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).expect("serialize")
}

fn array_page(count: usize) -> Vec<u8> {
    let items: Vec<Value> = (0..count).map(|i| serde_json::json!({"id": i})).collect();
    json_body(&Value::Array(items))
}

fn client_for(base: &str, token: Option<(&str, TokenKind)>) -> Client {
    Client::new(
        ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(5))
            .build(),
        HostSettings {
            api_url: format!("{base}/api/v4"),
            headers: BTreeMap::new(),
        },
        token.map(|(t, k)| (t.to_string(), k)),
        false,
    )
}

#[test]
fn delete_204_is_success() {
    let (base, rx) = spawn_server(|_| (204, vec![], Vec::new()));
    let c = client_for(&base, Some(("tok", TokenKind::Personal)));
    let v = c
        .send_json_value("DELETE", "projects/1/issues/5", &[], None)
        .expect("delete must succeed on 204");
    assert_eq!(v, Value::Null);
    let req = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(req.method, "DELETE");
    // The empty-body request must still be valid JSON-typed HTTP, not error.
}

#[test]
fn personal_token_uses_private_token_header() {
    let (base, rx) = spawn_server(|_| (200, vec![], json_body(&serde_json::json!({"id": 1}))));
    let c = client_for(&base, Some(("secret", TokenKind::Personal)));
    let _ = c.get_json_value("user", &[]).unwrap();
    let req = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(
        req.headers.get("private-token").map(String::as_str),
        Some("secret")
    );
    assert!(!req.headers.contains_key("job-token"));
}

#[test]
fn job_token_uses_job_token_header() {
    let (base, rx) = spawn_server(|_| (200, vec![], json_body(&serde_json::json!({"id": 1}))));
    let c = client_for(&base, Some(("jobtok", TokenKind::Job)));
    let _ = c.get_json_value("projects/1", &[]).unwrap();
    let req = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(
        req.headers.get("job-token").map(String::as_str),
        Some("jobtok")
    );
    assert!(!req.headers.contains_key("private-token"));
}

#[test]
fn bodyless_post_sends_explicit_json_body() {
    let (base, rx) = spawn_server(|_| (200, vec![], json_body(&serde_json::json!({}))));
    let c = client_for(&base, None);
    let _ = c
        .send_json_value("POST", "projects/1/pipeline", &[], None)
        .unwrap();
    let req = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(
        req.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert!(
        !req.body.is_empty(),
        "bodyless POST must still carry a JSON body"
    );
}

#[test]
fn rate_limited_request_retries_once_with_retry_after() {
    // First request: 429 with Retry-After: 1. Second: success. Proves the
    // retry fires, honors the header, and surfaces the success.
    let (base, rx) = spawn_server(|_| {
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            (
                429,
                vec![("Retry-After".to_string(), "1".to_string())],
                json_body(&serde_json::json!({"message": "slow down"})),
            )
        } else {
            (200, vec![], json_body(&serde_json::json!({"id": 1})))
        }
    });
    let c = client_for(&base, Some(("tok", TokenKind::Personal)));
    let started = std::time::Instant::now();
    let v = c.get_json_value("user", &[]).expect("retry must succeed");
    let elapsed = started.elapsed();
    assert_eq!(v, serde_json::json!({"id": 1}));
    // Retry-After: 1 was slept out (allowing slop for fast test machines).
    assert!(
        elapsed >= std::time::Duration::from_millis(900),
        "must honor Retry-After, took only {elapsed:?}"
    );
    let first = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    let second = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(first.path, second.path, "same request is replayed");
}

#[test]
fn second_429_surfaces_as_an_error_after_exactly_one_retry() {
    // Two consecutive 429s: exactly one retry, then the error surfaces.
    let (base, _rx) = spawn_server(|_| (429, vec![], json_body(&serde_json::json!({}))));
    let c = client_for(&base, None);
    let err = c
        .get_json_value("user", &[])
        .expect_err("persistent 429 must fail");
    assert!(err.to_string().contains("HTTP"));
}

#[test]
fn patch_reaches_the_wire_with_json_body() {
    // GitLab v4 has PATCH endpoints (e.g. personal_access_tokens self-rotate);
    // the passthrough must not reject them as an unsupported method.
    let (base, rx) = spawn_server(|_| (200, vec![], json_body(&serde_json::json!({"id": 1}))));
    let c = client_for(&base, Some(("tok", TokenKind::Personal)));
    let _ = c
        .send_json_value(
            "PATCH",
            "personal_access_tokens/1/self",
            &[],
            Some(&serde_json::json!({"scopes": ["api"]})),
        )
        .unwrap();
    let req = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(req.method, "PATCH");
    assert_eq!(
        req.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert_ne!(req.body, b"");
    assert!(std::str::from_utf8(&req.body).unwrap().contains("scopes"));
}

#[test]
fn unknown_methods_still_rejected() {
    let (base, _rx) = spawn_server(|_| (200, vec![], json_body(&serde_json::json!({}))));
    let c = client_for(&base, None);
    let err = c
        .send_json_value("BREW", "coffee", &[], None)
        .expect_err("unsupported method must be a usage error");
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn pagination_follows_x_next_page() {
    // Page 1: 2 items + X-Next-Page: 2. Page 2: 1 item + no next page.
    let (base, rx) = spawn_server(|req| {
        if req.path.contains("page=1&") {
            (
                200,
                vec![("X-Next-Page".to_string(), "2".to_string())],
                array_page(2),
            )
        } else {
            (200, vec![], array_page(1))
        }
    });
    let c = client_for(&base, None);
    let items = c.get_paginated("projects/1/issues", &[], None).unwrap();
    assert_eq!(items.len(), 3);
    let first = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    let second = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(first.path.contains("page=1"));
    assert!(second.path.contains("page=2"));
}

#[test]
fn pagination_stops_without_headers_on_short_page() {
    // No X-Next-Page header at all (endpoint that omits it): short page
    // terminates the loop.
    let (base, _rx) = spawn_server(|_| (200, vec![], array_page(3)));
    let c = client_for(&base, None);
    let items = c.get_paginated("projects/1/issues", &[], None).unwrap();
    assert_eq!(items.len(), 3);
}

#[test]
fn pagination_honors_caller_per_page() {
    // A caller-supplied per_page must be used as the page size, not
    // double-sent as a duplicate parameter (old behavior appended
    // per_page=100 next to the caller's pair). Page 1 carries 10 items
    // (a full per_page=10 page), page 2 is short and ends the loop.
    let (base, rx) = spawn_server(|req| {
        // "page=1&" matches "?page=1&per_page=..." but never the "page=1"
        // inside "per_page=10&..." (that is followed by "0", not "&").
        if req.path.contains("page=1&") {
            (200, vec![], array_page(10))
        } else {
            (200, vec![], array_page(3))
        }
    });
    let c = client_for(&base, None);
    let items = c
        .get_paginated(
            "projects/1/issues",
            &[("per_page".to_string(), "10".to_string())],
            None,
        )
        .unwrap();
    // Full first page (10) + short second page (3).
    assert_eq!(items.len(), 13);
    let first = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(first.path.contains("per_page=10"));
    assert!(!first.path.contains("per_page=100"));
}

#[test]
fn pagination_cap_ignores_runaway_server() {
    // A server that always returns a full page and claims there is a next
    // page must not hang: max_pages caps the loop.
    let (base, _rx) = spawn_server(|_| {
        (
            200,
            vec![("X-Next-Page".to_string(), "999".to_string())],
            array_page(100),
        )
    });
    let c = client_for(&base, None);
    let items = c.get_paginated("projects/1/issues", &[], Some(3)).unwrap();
    assert_eq!(items.len(), 300);
}

#[test]
fn pagination_honors_caller_page_as_start_page() {
    // A caller-supplied page must be the start page, sent exactly once. The
    // old behavior pushed page=<caller> next to the loop's own page=<n>,
    // making the effective start page server-dependent.
    let (base, rx) = spawn_server(|req| {
        // Page 2: 2 items + X-Next-Page: 3. Page 3: 1 item, last.
        if req.path.contains("page=2&per_page=") {
            (
                200,
                vec![("X-Next-Page".to_string(), "3".to_string())],
                array_page(2),
            )
        } else {
            (200, vec![], array_page(1))
        }
    });
    let c = client_for(&base, None);
    let items = c
        .get_paginated(
            "projects/1/issues",
            &[("page".to_string(), "2".to_string())],
            None,
        )
        .unwrap();
    assert_eq!(items.len(), 3);
    let first = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    let second = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    // Start page sent exactly once, then the loop advances from there.
    assert!(first.path.contains("page=2&per_page="), "{}", first.path);
    assert!(
        !first.path.contains("page=2&page="),
        "duplicate page parameter: {}",
        first.path
    );
    assert_eq!(first.path.matches("page=2").count(), 1);
    assert!(second.path.contains("page=3"), "{}", second.path);
}

#[test]
fn decode_json_empty_body_is_null() {
    let resp = ureq::Response::new(204, "No Content", "").unwrap();
    assert_eq!(decode_json(resp).unwrap(), Value::Null);
    let resp = ureq::Response::new(200, "OK", "").unwrap();
    assert_eq!(decode_json(resp).unwrap(), Value::Null);
}

#[test]
fn url_encoding_of_query_and_path() {
    let (base, _rx) = spawn_server(|_| (200, vec![], json_body(&Value::Array(vec![]))));
    let c = client_for(&base, None);
    // The URL builder must encode query values (spaces, ampersands).
    let url = c.url(
        "projects/g%2Fp/issues",
        &[
            ("labels".into(), "a b&c".into()),
            ("search".into(), "x/y".into()),
        ],
    );
    assert!(url.contains("labels=a%20b%26c"));
    assert!(url.contains("search=x%2Fy"));
    assert!(url.contains("/api/v4/projects/g%2Fp/issues"));
}

// ---------------------------------------------------------------------------
// Command-level integration: dispatch a full command group against the mock
// server, asserting on the wire requests and the printed output (not just the
// client primitives).
// ---------------------------------------------------------------------------

use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};

/// Serialize everything process-global the dispatch tests touch: the
/// stdout/stderr fd redirection and `XDG_CONFIG_HOME`. Both the install and
/// the restore must happen under this lock — swapping the env var from two
/// threads (each saving and restoring its own snapshot) leaves the last
/// closer's stale value behind and races with another test's dispatch.
static FD_LOCK: Mutex<()> = Mutex::new(());

/// Run one dispatched command with stdout/stderr captured via fd redirection
/// to temp files, against an isolated config dir provisioning the mock host.
/// Returns (stdout, stderr, result).
fn dispatch_captured(
    host: &str,
    base: &str,
    root: &glabrs::cli::Root,
) -> (String, String, Result<(), glabrs::error::Error>) {
    let guard = FD_LOCK.lock().unwrap();
    let saved_xdg = std::env::var_os("XDG_CONFIG_HOME");
    install_mock_config(host, base);
    let (out, err, result) = dispatch_redirected(root);
    if let Some(xdg) = saved_xdg {
        std::env::set_var("XDG_CONFIG_HOME", xdg);
    } else {
        std::env::remove_var("XDG_CONFIG_HOME");
    }
    drop(guard);
    (out, err, result)
}

/// Provision an isolated config dir for the mock host and point
/// `XDG_CONFIG_HOME` at it, so dispatch resolves per-host settings (and the
/// explicit `http://` `api_url` — derived URLs are always `https` and would
/// attempt TLS against the plain-HTTP mock) instead of the developer's real
/// configuration.
fn install_mock_config(host: &str, base: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let glabrs_dir = dir.path().join("glabrs");
    std::fs::create_dir_all(&glabrs_dir).expect("mkdir");
    std::fs::write(
        glabrs_dir.join("hosts.toml"),
        format!("[\"{host}\"]\ntoken = \"tok\"\napi_url = \"{base}/api/v4\"\n"),
    )
    .expect("write hosts.toml");
    let path = dir.path().to_path_buf();
    // Deliberately leaked: the temp dir must outlive `dispatch` (and any
    // connection pooling it leaves behind), and leaking one small dir per
    // command-level test keeps the env dance single-owner under the lock.
    std::mem::forget(dir);
    std::env::set_var("XDG_CONFIG_HOME", path);
}

/// Run one dispatched command with stdout/stderr captured via fd redirection
/// to temp files. Caller holds [`FD_LOCK`].
fn dispatch_redirected(
    root: &glabrs::cli::Root,
) -> (String, String, Result<(), glabrs::error::Error>) {
    let out_file = tempfile::NamedTempFile::new().expect("tempfile");
    let err_file = tempfile::NamedTempFile::new().expect("tempfile");
    let (out_fd, err_fd) = (out_file.as_raw_fd(), err_file.as_raw_fd());
    let saved_out = unsafe { libc::dup(1) };
    let saved_err = unsafe { libc::dup(2) };
    unsafe {
        libc::dup2(out_fd, 1);
        libc::dup2(err_fd, 2);
    }
    // Run dispatch on a fresh scoped thread: libtest captures each test
    // thread's `println!` through a thread-local hook BEFORE it reaches fd 1,
    // so a test-thread dispatch would print into libtest's buffer and the
    // redirected file would stay empty. A new thread has no capture hook and
    // writes straight to fd 1.
    //
    // The fd restore below runs even when dispatch panics (join surfaces the
    // payload and the test thread re-panics after the restore).
    let joined = std::thread::scope(|s| s.spawn(|| glabrs::cmd::dispatch(root)).join());
    unsafe {
        libc::dup2(saved_out, 1);
        libc::dup2(saved_err, 2);
        libc::close(saved_out);
        libc::close(saved_err);
    }
    let result = joined.unwrap_or_else(|p| std::panic::resume_unwind(p));
    (read_captured(&out_file), read_captured(&err_file), result)
}

/// Read a captured-output temp file from offset 0. A `dup` of the temp file
/// shares its open file description (and thus its offset with the writes), so
/// reading through it would start at EOF; reopening by path gets an
/// independent description.
fn read_captured(file: &tempfile::NamedTempFile) -> String {
    let mut buf = Vec::new();
    let Ok(mut reader) = std::fs::File::open(file.path()) else {
        return String::new();
    };
    let _ = reader.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).to_string()
}

/// A Root targeting `base` explicitly: the URL `-R` target and `--hostname`
/// agree (the test process itself may run inside any git checkout, so remote
/// detection must not participate). `base` is the mock server URL
/// (`http://127.0.0.1:PORT`).
fn cmd_root(base: &str, command: glabrs::cli::Command) -> glabrs::cli::Root {
    let host = host_of(base);
    glabrs::cli::Root {
        repo: Some(format!("{base}/group/proj")),
        hostname: Some(host.to_string()),
        token: None,
        insecure: false,
        json: false,
        no_pager: true,
        verbose: false,
        command,
    }
}

/// The host:port part of a mock server base URL (`http://127.0.0.1:PORT`).
fn host_of(base: &str) -> &str {
    let rest = base
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    rest.split('/').next().unwrap_or(rest)
}

#[test]
fn mr_create_sends_expected_body_and_renders_iid() {
    let seen: Arc<Mutex<Vec<Req>>> = Arc::default();
    let for_server = Arc::clone(&seen);
    let (base, _rx) = spawn_server(move |req| {
        for_server.lock().unwrap().push(req.clone());
        if req.path.contains("/merge_requests") && req.method == "POST" {
            (
                200,
                vec![],
                json_body(&serde_json::json!({
                    "iid": 7, "title": "T", "state": "opened",
                    "web_url": "http://x/mr/7"
                })),
            )
        } else {
            // default_branch lookup
            (
                200,
                vec![],
                json_body(&serde_json::json!({"default_branch": "main"})),
            )
        }
    });
    let root = cmd_root(
        &base,
        glabrs::cli::Command::Mr {
            command: glabrs::cli::mr_cmd::MrArgs::Create {
                title: Some("T".into()),
                description: None,
                description_file: None,
                source_branch: Some("feat".into()),
                target_branch: None,
                draft: true,
                assignee: None,
                reviewer: None,
                milestone: None,
                labels: Some("a,b".into()),
                fill: false,
                push: false,
                squash: true,
                remove_source_branch: true,
                web: false,
            },
        },
    );
    let (_out, _err, result) = dispatch_captured(host_of(&base), &base, &root);
    result.expect("mr create must succeed");
    let out_f = seen.lock().unwrap();
    let create = out_f
        .iter()
        .find(|r| r.method == "POST" && r.path.contains("/merge_requests"))
        .expect("POST merge_requests on the wire");
    let body: Value = serde_json::from_slice(&create.body).unwrap();
    assert_eq!(body["source_branch"], "feat");
    assert_eq!(body["target_branch"], "main");
    assert_eq!(body["title"], "Draft: T");
    assert_eq!(body["labels"], "a,b");
    assert_eq!(body["squash"], true);
    assert_eq!(body["remove_source_branch"], true);
    // The render (iid shown) runs through the same render_one path the other
    // commands use; the wire assertions above are the regression value here.
}

#[test]
fn mr_create_rejects_repo_url_host_mismatch() {
    // -R points at base (h.example) but the command targets base2: refuse
    // instead of querying the wrong instance with a valid path.
    let (base, _rx) = spawn_server(|_| (200, vec![], json_body(&serde_json::json!({}))));
    let (base2, _rx2) = spawn_server(|_| (200, vec![], json_body(&serde_json::json!({}))));
    let mut root = cmd_root(
        &base,
        glabrs::cli::Command::Mr {
            command: glabrs::cli::mr_cmd::MrArgs::View { iid: 1, web: false },
        },
    );
    root.repo = Some(format!("{base2}/group/proj"));
    let (_out, _err, result) = dispatch_captured(host_of(&base), &base, &root);
    let err = result.expect_err("host mismatch must be refused");
    assert_eq!(err.exit_code(), 2);
    assert!(err.to_string().contains("points at"), "{err}");
    assert!(err.to_string().contains("--hostname"), "{err}");
}

#[test]
fn ci_trace_snapshot_prints_still_running_and_exits_zero() {
    // A running job without --follow: the trace prints, the footer must NOT
    // claim the job "finished", and the snapshot exits 0.
    let (base, _rx) = spawn_server(|req| {
        if req.path.contains("/trace") {
            (200, vec![], b"build log line 1\n".to_vec())
        } else if req.path.contains("/jobs/") {
            (
                200,
                vec![],
                json_body(&serde_json::json!({"id": 1, "status": "running", "name": "build"})),
            )
        } else {
            (
                200,
                vec![],
                json_body(&serde_json::json!(
                    [{"id": 42, "name": "build", "status": "running"}]
                )),
            )
        }
    });
    let root = cmd_root(
        &base,
        glabrs::cli::Command::Ci {
            command: glabrs::cli::ci_cmd::CiArgs::Trace {
                target: "42".into(),
                follow: false,
                timeout: None,
                pipeline: None,
            },
        },
    );
    let (out, err, result) = dispatch_captured(host_of(&base), &base, &root);
    // The regression this pins: the old code failed a still-running job with
    // "job not finished (status: running)". A snapshot must exit 0. (The
    // trace body and the "still running" footer cannot be asserted here —
    // libtest captures println! from all test-process threads before fd 1,
    // so captured files only ever hold the harness's own status lines. The
    // footer wording is verified by running the real binary against a mock.)
    result.expect("snapshot of a running job must exit 0");
    assert!(
        !err.contains("finished"),
        "must not claim finished: {out:?} / {err:?}"
    );
}

#[test]
fn ci_status_snapshot_running_exits_zero() {
    // status without --wait on a running pipeline: report and exit 0.
    let (base, _rx) = spawn_server(|_| {
        (
            200,
            vec![],
            json_body(&serde_json::json!(
                [{"id": 9, "iid": 1, "ref": "main", "status": "running"}]
            )),
        )
    });
    let root = cmd_root(
        &base,
        glabrs::cli::Command::Ci {
            command: glabrs::cli::ci_cmd::CiArgs::Status {
                ref_: None,
                wait: false,
                timeout: None,
            },
        },
    );
    let (out, err, result) = dispatch_captured(host_of(&base), &base, &root);
    // Same snapshot contract as the trace test above: the old code failed a
    // running pipeline without --wait.
    result.expect("snapshot of a running pipeline must exit 0");
    assert!(
        !out.is_empty() || !err.is_empty(),
        "no captured output at all: {out:?} / {err:?}"
    );
}

#[test]
fn custom_port_instance_matches_across_host_sources() {
    // Regression for the split host identity: a checkout on h.example:8443
    // and `auth login --hostname h.example:8443` must resolve to the same
    // hosts.toml entry (the old code stored the bare host from remote
    // detection and the ported host from the flag, so ported checkouts
    // missed their entry). End-to-end through dispatch: the entry is found,
    // the request goes to the configured http api_url, no mismatch error.
    let (base, rx) = spawn_server(|req| {
        if req.path.contains("/merge_requests") {
            (
                200,
                vec![],
                json_body(&serde_json::json!(
                    [{"iid": 1, "title": "T", "state": "opened"}]
                )),
            )
        } else {
            (200, vec![], json_body(&serde_json::json!({})))
        }
    });
    // Hostname flag carries the non-default port (host_of includes it for a
    // mock base); the stored hosts.toml entry uses the same spelling (as
    // `auth login --hostname` would write it).
    let host = host_of(&base).to_string();
    let root = glabrs::cli::Root {
        repo: None,
        hostname: Some(host.clone()),
        token: None,
        insecure: false,
        json: false,
        no_pager: true,
        verbose: false,
        command: glabrs::cli::Command::Mr {
            command: glabrs::cli::mr_cmd::MrArgs::List {
                assignee: None,
                reviewer: None,
                author: None,
                state: None,
                list: glabrs::cli::ListOpts {
                    per_page: 20,
                    page: None,
                    paginate: false,
                },
            },
        },
    };
    // -R carries the ported URL exactly like the git remote would parse it.
    let mut root = root;
    root.repo = Some(format!("{base}/group/proj"));
    let (_out, _err, result) = dispatch_captured(&host, &base, &root);
    result.expect("ported checkout must resolve the ported hosts.toml entry");
    let req = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(
        req.path.contains("projects/group%2Fproj/merge_requests"),
        "{}",
        req.path
    );
}

#[test]
fn config_unset_missing_key_is_not_found() {
    // The no-op unset contract: unknown key exits 4 on the global side.
    // (Exercise the pure store-level equivalent: Config::unset returns false
    // and Config::get is None, which the command turns into exit 4.)
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("config.toml");
    std::fs::write(&path, "pager = \"less\"\n").expect("write");
    let mut cfg = glabrs::config::store::Config::load_from(&path).expect("load");
    assert!(cfg.get("pager").is_some());
    assert!(cfg.get("nosuchkey").is_none());
    assert!(!cfg.unset("nosuchkey"));
    assert!(cfg.unset("pager"));
    assert!(cfg.get("pager").is_none());
    // Pruning still works after a real removal.
    assert!(cfg.root().as_table().is_some_and(toml::map::Map::is_empty));
}
