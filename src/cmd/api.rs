use serde_json::{json, Value};

use crate::cli::api_cmd::ApiArgs;
use crate::cli::Root;
use crate::cmd::resolve;
use crate::error::Error;

pub fn run(root: &Root, args: &ApiArgs) -> Result<(), Error> {
    let ctx = resolve(root, root.hostname.as_deref())?;
    let mut query: Vec<(String, String)> = Vec::new();

    // -F style: ":id=value" sets a path field (replaces :id in the path);
    // "key=value" becomes a query parameter.
    let mut path_fields: std::collections::BTreeMap<String, String> = Default::default();
    for f in &args.fields {
        match f.split_once('=') {
            Some((k, v)) if k.starts_with(':') => {
                path_fields.insert(k[1..].to_string(), v.to_string());
            }
            Some((k, v)) => query.push((k.to_string(), v.to_string())),
            None => return Err(Error::usage(format!("bad --field {f:?} (want key=value)"))),
        }
    }
    let mut path = args.path.clone();
    for (k, v) in &path_fields {
        path = path.replace(&format!(":{k}"), &urlencode_path_field(v));
    }

    let body = match &args.input {
        Some(p) => {
            let text = crate::render::pager::read_input(p)?;
            Some(
                serde_json::from_str::<Value>(&text)
                    .map_err(|e| Error::usage(format!("--input must be JSON: {e}")))?,
            )
        }
        None => None,
    };

    if args.paginate {
        if args.method != "GET" {
            return Err(Error::usage("--paginate requires GET"));
        }
        let items = ctx.client().get_paginated(&path, &query, None)?;
        crate::render::table::print_json(&Value::Array(items))?;
    } else {
        let value = ctx
            .client()
            .send_json_value(&args.method, &path, &query, body)?;
        // 204 No Content and other empty responses decode to null; report a
        // small sentinel so `jq` gets valid JSON on stdout.
        if value.is_null() {
            let status = if args.method == "DELETE" {
                "deleted"
            } else {
                "ok"
            };
            crate::render::table::print_json(&json!({ "status": status }))?;
        } else {
            crate::render::table::print_json(&value)?;
        }
    }
    Ok(())
}

/// Path substitution values are percent-encoded except `/` and `%`, which the
/// caller controls via an already-encoded path (mirrors glab's `-F` behavior
/// loosely but stays safe for spaces and unicode).
fn urlencode_path_field(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for b in v.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b'%' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
