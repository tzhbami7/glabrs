use serde_json::{json, Value};

use crate::cli::api_cmd::ApiArgs;
use crate::cli::Root;
use crate::cmd::resolve;
use crate::error::Error;

/// Run one `api` passthrough request.
///
/// # Errors
///
/// Returns [`Error::Usage`] for malformed fields or invalid flag
/// combinations and [`Error`] when the request fails.
pub fn run(root: &Root, args: &ApiArgs) -> Result<(), Error> {
    // Validate and build the request before resolving the context: usage
    // errors must not depend on a resolvable git remote or host config.
    let mut query: Vec<(String, String)> = Vec::new();

    // -F style: ":id=value" sets a path field (replaces :id in the path);
    // "key=value" becomes a query parameter.
    let mut path_fields: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
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
        let placeholder = format!(":{k}");
        if !path.contains(&placeholder) {
            return Err(Error::usage(format!(
                "--field :{k} has no matching :{k} placeholder in the path"
            )));
        }
        path = path.replace(&placeholder, &crate::api::client::urlencode_with(v, true));
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

    // Same rule as clap's `requires`: `--page` needs `--paginate`, and
    // pagination is GET-only.
    if let Some(p) = args.page {
        if !args.paginate {
            return Err(Error::usage("--page requires --paginate"));
        }
        if args.method != "GET" {
            return Err(Error::usage("--paginate requires GET"));
        }
        query.push(("page".into(), p.to_string()));
    }

    let ctx = resolve(root, root.hostname.as_deref())?;

    if args.paginate {
        if args.method != "GET" {
            return Err(Error::usage("--paginate requires GET"));
        }
        let items = ctx.client().get_paginated(&path, &query, None)?;
        crate::render::table::print_json(&Value::Array(items))?;
    } else {
        let value = ctx
            .client()
            .send_json_value(&args.method, &path, &query, body.as_ref())?;
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

#[cfg(test)]
mod tests {
    use crate::api::client::urlencode_with;

    #[test]
    fn path_field_encoding() {
        assert_eq!(urlencode_with("a b", true), "a%20b");
        assert_eq!(urlencode_with("g/p", true), "g/p");
        assert_eq!(urlencode_with("100%", true), "100%");
    }
}
