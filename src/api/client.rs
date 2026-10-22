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
    Personal,
    Job,
}

/// Blocking API client for a single GitLab instance.
pub struct Client {
    agent: Agent,
    pub api_url: String,
    headers: BTreeMap<String, String>,
    token: Option<(String, TokenKind)>,
    pub verbose: bool,
}

/// Resolved connection settings for one host.
pub struct HostSettings {
    pub api_url: String,
    pub headers: BTreeMap<String, String>,
}

impl Client {
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
    pub fn token_kind(&self) -> Option<TokenKind> {
        self.token.as_ref().map(|(_, k)| *k)
    }

    /// Additional headers to apply on raw (non-`call`) requests.
    pub fn extra_headers(&self) -> &BTreeMap<String, String> {
        &self.headers
    }

    /// GET a paginated list, following `X-Next-Page` until exhausted or
    /// `max_pages` is reached (default 10,000 pages).
    ///
    /// Stops on the `X-Next-Page` header when the server sends one; falls back
    /// to "short page means last" for endpoints that omit pagination headers.
    /// The hard cap guarantees termination even against a server that ignores
    /// the `page` parameter.
    pub fn get_paginated(
        &self,
        path: &str,
        query: &[(String, String)],
        max_pages: Option<u32>,
    ) -> Result<Vec<Value>, Error> {
        let mut all = Vec::new();
        let cap = max_pages.unwrap_or(MAX_PAGES_DEFAULT);
        let mut page: u32 = 1;
        loop {
            let mut q = query.to_vec();
            q.push(("page".into(), page.to_string()));
            q.push(("per_page".into(), "100".into()));
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
                        if other.is_object() { "object" } else { "scalar" }
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
                    if count < 100 || page >= cap {
                        break;
                    }
                    page += 1;
                }
            }
        }
        Ok(all)
    }

    pub fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(String, String)],
    ) -> Result<T, Error> {
        let value = self.get_json_value(path, query)?;
        serde_json::from_value(value).map_err(Error::from)
    }

    pub fn get_json_value(&self, path: &str, query: &[(String, String)]) -> Result<Value, Error> {
        let url = self.url(path, query);
        self.verbose_log("GET", path);
        let resp = self.apply_auth(self.agent.get(&url)).call()?;
        decode_json(resp)
    }

    /// GET a JSON array (single page).
    pub fn get_json_vec(
        &self,
        path: &str,
        query: &[(String, String)],
    ) -> Result<Vec<Value>, Error> {
        self.get_json(path, query)
    }

    /// Raw GET returning a request (for traces and file downloads).
    pub fn agent_get(&self, url: &str) -> Result<ureq::Request, Error> {
        Ok(self.apply_auth(self.agent.get(url)))
    }

    /// Raw POST returning a request (for multipart uploads).
    pub fn agent_post(&self, url: &str) -> Result<ureq::Request, Error> {
        Ok(self.apply_auth(self.agent.post(url)))
    }

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
pub fn host_of_api_url(api_url: &str) -> Option<String> {
    let u = url::Url::parse(api_url).ok()?;
    u.host_str().map(|s| s.to_ascii_lowercase())
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
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
}
