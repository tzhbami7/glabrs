use std::collections::BTreeMap;
use std::io::Read;

use serde_json::Value;
use ureq::Agent;

use crate::error::Error;

/// Hard cap on `--paginate` so a server that keeps returning full pages
/// cannot make the client loop forever.
const MAX_PAGES_DEFAULT: u32 = 10_000;

/// How the token is presented to the API.
///
/// Personal access tokens use `PRIVATE-TOKEN`; CI job tokens must use
/// `JOB-TOKEN` (and only authenticate project-scoped endpoints).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Personal access token, sent as `PRIVATE-TOKEN`.
    Personal,
    /// CI job token, sent as `JOB-TOKEN` (project-scoped endpoints only).
    Job,
}

/// Blocking API client for a single GitLab instance.
///
/// Cloning shares the underlying connection pool ([`Agent`]); one client is
/// built per invocation by [`resolve`](crate::cmd::resolve).
pub struct Client {
    agent: Agent,
    /// The resolved API base URL (`https://host[:port]/prefix/api/v4`).
    pub api_url: String,
    headers: BTreeMap<String, String>,
    token: Option<(String, TokenKind)>,
    /// When set, `verbose_log` prints API method and path (never headers).
    pub verbose: bool,
}

/// Resolved connection settings for one host.
pub struct HostSettings {
    /// The API base URL for the host.
    pub api_url: String,
    /// Additional headers applied to every request.
    pub headers: BTreeMap<String, String>,
}

impl Client {
    /// Build a client from an agent, resolved host settings, and optional
    /// token (with its [`TokenKind`]).
    #[must_use]
    pub fn new(
        agent: Agent,
        settings: HostSettings,
        token: Option<(String, TokenKind)>,
        verbose: bool,
    ) -> Self {
        Self {
            agent,
            api_url: settings.api_url,
            headers: settings.headers,
            token,
            verbose,
        }
    }

    /// The token kind in use, if any.
    #[must_use]
    pub fn token_kind(&self) -> Option<TokenKind> {
        self.token.as_ref().map(|(_, k)| *k)
    }

    /// Additional headers applied to every request (per-host `headers`).
    ///
    /// Kept as API surface for the lib target; the CLI currently reaches the
    /// per-host headers through `Client::new` only. (Public items in a lib
    /// crate are never `dead_code`, so no suppression is needed here.)
    #[must_use]
    pub fn extra_headers(&self) -> &BTreeMap<String, String> {
        &self.headers
    }

    /// GET a paginated list, following `X-Next-Page` until exhausted or
    /// `max_pages` is reached (default 10,000 pages).
    ///
    /// `per_page` and `page` pairs in `query` (e.g. from `--per-page` /
    /// `--page`) are honored as the page size and start page and not
    /// double-sent as duplicate parameters (GitLab rejects duplicates, and a
    /// duplicated `page` makes the start page server-dependent). Stops on the
    /// `X-Next-Page` header when the server sends one; falls back to "short
    /// page means last" for endpoints that omit pagination headers. The hard
    /// cap guarantees termination even against a server that ignores the
    /// `page` parameter.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when a request fails (HTTP status mapped to
    /// [`Error::Auth`] for 401) or the response is not a JSON array.
    pub fn get_paginated(
        &self,
        path: &str,
        query: &[(String, String)],
        max_pages: Option<u32>,
    ) -> Result<Vec<Value>, Error> {
        // Extract the caller's page size instead of appending a duplicate
        // per_page parameter next to it (GitLab rejects duplicates).
        let per_page = query
            .iter()
            .find(|(k, _)| k == "per_page")
            .and_then(|(_, v)| v.trim().parse::<u32>().ok())
            .filter(|n| (1..=100).contains(n))
            .unwrap_or(100);
        // Honor a caller page as the start page instead of appending a
        // duplicate page parameter next to it (the effective start page would
        // otherwise be server-dependent). Kept as a defensive filter: the CLI
        // validates `--page >= 1`, but this function is also called directly
        // (tests, future callers).
        let start_page = query
            .iter()
            .find(|(k, _)| k == "page")
            .and_then(|(_, v)| v.trim().parse::<u32>().ok())
            .filter(|n| *n >= 1);
        let query: Vec<(String, String)> = query
            .iter()
            .filter(|(k, _)| k != "per_page" && k != "page")
            .cloned()
            .collect();
        let mut all = Vec::new();
        let cap = max_pages.unwrap_or(MAX_PAGES_DEFAULT);
        let mut page: u32 = start_page.unwrap_or(1);
        // Page parameters appended once and rewritten in place each iteration:
        // re-cloning the query per page allocates the same buffer over and over.
        let mut q = query;
        q.push(("page".into(), String::new()));
        q.push(("per_page".into(), per_page.to_string()));
        let page_idx = q.len() - 2;
        loop {
            q[page_idx].1 = page.to_string();
            let url = self.url(path, &q);
            self.verbose_log("GET", path);
            let resp = self.apply_auth(self.agent.get(&url)).call()?;
            let next_page = resp
                .header("X-Next-Page")
                .and_then(|v| v.trim().parse::<u32>().ok());
            let items = match decode_json(resp)? {
                Value::Array(a) => a,
                Value::Null => Vec::new(),
                other => {
                    return Err(Error::runtime(format!(
                        "expected a JSON array from {path}, got {}",
                        if other.is_object() {
                            "object"
                        } else {
                            "scalar"
                        }
                    )))
                }
            };
            let count = items.len();
            all.extend(items);
            match next_page {
                // Server told us to continue: advance one page at a time so a
                // bogus header value can never jump past the cap.
                Some(n) if n > page && page < cap => page += 1,
                Some(_) => break,
                // No header: fall back to short-page detection, capped.
                None => {
                    if count < per_page as usize || page >= cap {
                        break;
                    }
                    page += 1;
                }
            }
        }
        Ok(all)
    }

    /// GET a single resource and deserialize it.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the request fails or the body does not match `T`.
    pub fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(String, String)],
    ) -> Result<T, Error> {
        let value = self.get_json_value(path, query)?;
        serde_json::from_value(value).map_err(Error::from)
    }

    /// GET a single resource as a raw [`Value`].
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the request fails or the body is not JSON.
    pub fn get_json_value(&self, path: &str, query: &[(String, String)]) -> Result<Value, Error> {
        let url = self.url(path, query);
        self.verbose_log("GET", path);
        let resp = self.apply_auth(self.agent.get(&url)).call()?;
        decode_json(resp)
    }

    /// GET a JSON array (single page).
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the request fails or the body is not an array.
    pub fn get_json_vec(
        &self,
        path: &str,
        query: &[(String, String)],
    ) -> Result<Vec<Value>, Error> {
        self.get_json(path, query)
    }

    /// Raw GET returning a request (for traces and file downloads).
    ///
    /// # Errors
    ///
    /// Currently infallible; returns `Result` so callers stay compatible with
    /// the fallible request helpers.
    pub fn agent_get(&self, url: &str) -> Result<ureq::Request, Error> {
        Ok(self.apply_auth(self.agent.get(url)))
    }

    /// Raw POST returning a request (for multipart uploads).
    ///
    /// # Errors
    ///
    /// Currently infallible; returns `Result` so callers stay compatible with
    /// the fallible request helpers.
    pub fn agent_post(&self, url: &str) -> Result<ureq::Request, Error> {
        Ok(self.apply_auth(self.agent.post(url)))
    }

    /// Send a JSON body (or an explicit empty JSON object) and decode the
    /// response.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Usage`] for unsupported methods and [`Error`] for
    /// request or decode failures.
    pub fn send_json_value(
        &self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        body: Option<Value>,
    ) -> Result<Value, Error> {
        let url = self.url(path, query);
        self.verbose_log(method, path);
        let req = match method {
            "GET" => self.agent.get(&url),
            "POST" => self.agent.post(&url),
            "PUT" => self.agent.put(&url),
            "DELETE" => self.agent.delete(&url),
            _ => return Err(Error::usage(format!("unsupported method {method}"))),
        };
        let req = self.apply_auth(req);
        let resp = match body {
            Some(b) => req.send_json(b)?,
            // Send an explicit empty JSON object: ureq's `call()` would inherit
            // the application/json content-type from `send_json` elsewhere in
            // the chain while carrying no payload, which stricter servers and
            // reverse proxies reject — and `send_string` alone would set
            // text/plain.
            None => req
                .set("Content-Type", "application/json")
                .send_string("{}")?,
        };
        decode_json(resp)
    }

    /// Build the request URL: `api_url` + path + percent-encoded query string.
    #[must_use]
    pub fn url(&self, path: &str, query: &[(String, String)]) -> String {
        let base = self.api_url.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let mut s = format!("{base}/{path}");
        if !query.is_empty() {
            s.push('?');
            s.push_str(
                &query
                    .iter()
                    .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
                    .collect::<Vec<_>>()
                    .join("&"),
            );
        }
        s
    }

    fn apply_auth(&self, req: ureq::Request) -> ureq::Request {
        let mut req = req;
        for (k, v) in &self.headers {
            req = req.set(k, v);
        }
        if let Some((t, kind)) = &self.token {
            let header = match kind {
                TokenKind::Personal => "PRIVATE-TOKEN",
                TokenKind::Job => "JOB-TOKEN",
            };
            req = req.set(header, t);
        }
        req
    }

    fn verbose_log(&self, method: &str, path: &str) {
        if self.verbose {
            eprintln!("api: {method} {path}");
            // Never print headers or token.
        }
    }
}

/// Decode a successful response, treating 204 / empty bodies as `null`.
///
/// GitLab returns 204 No Content for several endpoints (delete issue/release,
/// some approve flows); parsing those as JSON is always a spurious failure.
///
/// # Errors
///
/// Returns [`Error`] when reading the body fails or the body is not JSON.
pub fn decode_json(resp: ureq::Response) -> Result<Value, Error> {
    if resp.status() == 204 {
        return Ok(Value::Null);
    }
    let len = resp
        .header("Content-Length")
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(1);
    if len == 0 {
        return Ok(Value::Null);
    }
    let mut text = String::new();
    resp.into_reader().read_to_string(&mut text)?;
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_str(&text)?)
}

/// Extract the hostname from an API URL, if parseable.
#[must_use]
pub fn host_of_api_url(api_url: &str) -> Option<String> {
    let u = url::Url::parse(api_url).ok()?;
    u.host_str().map(str::to_ascii_lowercase)
}

/// Reduce an API URL to the instance root:
/// `https://host[:port]/prefix/api/v4` -> `https://host[:port]/prefix`.
///
/// `/uploads` and web URLs live at instance scope, and the `api_url` (unlike a
/// project web URL) ends in a known `/api/v4` suffix, so the prefix boundary
/// is unambiguous. Without the suffix, fall back to scheme + `host[:port]`
/// (parsed, so http-only instances keep their scheme).
///
/// # Examples
///
/// ```
/// use glabrs::api::client::instance_root;
///
/// assert_eq!(instance_root("https://h.example/gitlab/api/v4"), "https://h.example/gitlab");
/// assert_eq!(instance_root("http://localhost:8080/api/v4"), "http://localhost:8080");
/// ```
#[must_use]
pub fn instance_root(api_url: &str) -> String {
    let trimmed = api_url.trim_end_matches('/');
    if let Some(base) = trimmed.strip_suffix("/api/v4") {
        if !base.is_empty() {
            return base.to_string();
        }
    }
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

/// Percent-encode a string. `allow_path` additionally keeps `/` and `%`
/// unescaped, for path segments the caller controls via an already-encoded
/// path (mirrors glab's `-F` behavior loosely but stays safe for spaces and
/// unicode). Query strings and query values use the strict form.
///
/// # Examples
///
/// ```
/// use glabrs::api::client::urlencode_with;
///
/// assert_eq!(urlencode_with("a b", false), "a%20b");
/// // Path form keeps / and % (caller-controlled encoding).
/// assert_eq!(urlencode_with("g/p", true), "g/p");
/// assert_eq!(urlencode_with("100%", true), "100%");
/// ```
#[must_use]
pub fn urlencode_with(s: &str, allow_path: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b'/' | b'%' if allow_path => out.push(b as char),
            _ => {
                // Hex digits written directly into the output: an intermediate
                // String per escaped byte allocates on every URL build.
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(HEX[usize::from(b >> 4)] as char);
                out.push(HEX[usize::from(b & 0x0F)] as char);
            }
        }
    }
    out
}

/// Strict percent-encoding (query strings and query values).
///
/// # Examples
///
/// ```
/// use glabrs::api::client::urlencode;
///
/// assert_eq!(urlencode("a b&c"), "a%20b%26c");
/// assert_eq!(urlencode("x/y"), "x%2Fy");
/// ```
#[must_use]
pub fn urlencode(s: &str) -> String {
    urlencode_with(s, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> Client {
        use std::time::Duration;
        Client::new(
            ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(5))
                .build(),
            HostSettings {
                api_url: "https://gitlab.example/api/v4".into(),
                headers: BTreeMap::new(),
            },
            None,
            false,
        )
    }

    #[test]
    fn url_encoding() {
        let c = test_client();
        let url = c.url("projects/x%2Fy/issues", &[("labels".into(), "a,b".into())]);
        assert!(url.contains("labels=a%2Cb"));
        assert!(url.starts_with("https://gitlab.example/api/v4/projects/x%2Fy/issues?"));
    }

    #[test]
    fn urlencode_path_keeps_slashes_and_percent() {
        // Path-substitution form keeps / and % (caller-controlled encoding);
        // query form escapes them.
        assert_eq!(urlencode_with("a b/c%d", true), "a%20b/c%d");
        assert_eq!(urlencode_with("a b/c%d", false), "a%20b%2Fc%25d");
    }

    #[test]
    fn decode_json_nulls() {
        // 204 No Content and empty bodies decode to null, not an error.
        let resp = ureq::Response::new(204, "No Content", "").unwrap();
        assert_eq!(decode_json(resp).unwrap(), Value::Null);
        let resp = ureq::Response::new(200, "OK", "").unwrap();
        assert_eq!(decode_json(resp).unwrap(), Value::Null);
    }

    #[test]
    fn decode_json_objects() {
        let resp = ureq::Response::new(200, "OK", r#"{"id":7}"#).unwrap();
        assert_eq!(decode_json(resp).unwrap(), serde_json::json!({"id": 7}));
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
        // No /api/v4 suffix: scheme + host[:port] from the URL itself.
        assert_eq!(
            instance_root("http://gitlab.internal:8080/custom"),
            "http://gitlab.internal:8080"
        );
    }
}
