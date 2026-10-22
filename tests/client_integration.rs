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
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let req = read_request(&mut stream).expect("read request");
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
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "OK",
    };
    let mut resp = format!("HTTP/1.1 {status} {reason}\r\n");
    for (k, v) in headers {
        resp.push_str(&format!("{k}: {v}\r\n"));
    }
    resp.push_str(&format!("Content-Length: {}\r\n", body.len()));
    resp.push_str("Connection: close\r\n\r\n");
    stream.write_all(resp.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn json_body(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).expect("serialize")
}

fn array_page(count: usize) -> Vec<u8> {
    let items: Vec<Value> = (0..count)
        .map(|i| serde_json::json!({"id": i}))
        .collect();
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
    assert_eq!(req.headers.get("private-token").map(String::as_str), Some("secret"));
    assert!(!req.headers.contains_key("job-token"));//
}

#[test]
fn job_token_uses_job_token_header() {
    let (base, rx) = spawn_server(|_| (200, vec![], json_body(&serde_json::json!({"id": 1}))));
    let c = client_for(&base, Some(("jobtok", TokenKind::Job)));
    let _ = c.get_json_value("projects/1", &[]).unwrap();
    let req = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(req.headers.get("job-token").map(String::as_str), Some("jobtok"));
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
    assert!(!req.body.is_empty(), "bodyless POST must still carry a JSON body");
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
        &[("labels".into(), "a b&c".into()), ("search".into(), "x/y".into())],
    );
    assert!(url.contains("labels=a%20b%26c"));
    assert!(url.contains("search=x%2Fy"));
    assert!(url.contains("/api/v4/projects/g%2Fp/issues"));
}
