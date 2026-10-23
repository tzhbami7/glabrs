use serde_json::Value;
use tabled::builder::Builder;
use tabled::settings::Style;

use crate::error::Error;

/// Render a list of items as a table or raw JSON.
///
/// Empty lists print `No results` (not an error — an empty result is normal).
///
/// # Errors
///
/// Returns [`Error`] when JSON serialization fails.
pub fn render_list(items: &[Value], headers: &[&str], json_mode: bool) -> Result<(), Error> {
    if json_mode {
        print_json(&Value::Array(items.to_vec()))?;
        return Ok(());
    }
    if items.is_empty() {
        println!("No results");
        return Ok(());
    }
    let mut builder = Builder::new();
    builder.push_record(
        headers
            .iter()
            .map(|h| h.replace('.', " "))
            .collect::<Vec<_>>(),
    );
    for it in items {
        builder.push_record(headers.iter().map(|h| cell_str(it, h)).collect::<Vec<_>>());
    }
    println!("{}", builder.build().with(Style::markdown()));
    Ok(())
}

/// Render a single item as field/value rows.
///
/// # Errors
///
/// Returns [`Error`] when JSON serialization fails.
pub fn render_one(item: &Value, fields: &[&str], json_mode: bool) -> Result<(), Error> {
    if json_mode {
        print_json(item)?;
        return Ok(());
    }
    let mut builder = Builder::new();
    builder.push_record(vec!["Field".to_string(), "Value".to_string()]);
    for f in fields {
        builder.push_record(vec![f.to_string(), cell_str(item, f)]);
    }
    println!("{}", builder.build().with(Style::markdown()));
    Ok(())
}

/// Print a value as pretty JSON on stdout (the scripting-friendly form).
///
/// # Errors
///
/// Returns [`Error`] when serialization fails.
pub fn print_json(v: &Value) -> Result<(), Error> {
    println!(
        "{}",
        serde_json::to_string_pretty(v).map_err(|e| Error::runtime(format!("json: {e}")))?
    );
    Ok(())
}

/// Render one cell value. Multi-line content (descriptions, bodies) is
/// collapsed so it stays readable inside a one-row-per-field table; long
/// values are truncated with an ellipsis rather than blowing out the layout.
fn cell_str(item: &Value, key: &str) -> String {
    let value = raw_cell(item, key);
    let single_line = value.replace('\n', " ⏎ ");
    if single_line.chars().count() > MAX_CELL_CHARS {
        let truncated: String = single_line.chars().take(MAX_CELL_CHARS).collect();
        format!("{truncated}…")
    } else {
        single_line
    }
}

const MAX_CELL_CHARS: usize = 300;

fn raw_cell(item: &Value, key: &str) -> String {
    // Supports dotted paths like "author.username".
    let mut cur = item;
    for part in key.split('.') {
        match cur.get(part) {
            Some(v) => cur = v,
            None => return "-".to_string(),
        }
    }
    match cur {
        Value::Null => "-".to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => "<object>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cell_str_dotted_paths() {
        let v = json!({"author": {"username": "amy"}, "labels": ["a", "b"], "n": 3});
        assert_eq!(cell_str(&v, "author.username"), "amy");
        assert_eq!(cell_str(&v, "labels"), "a, b");
        assert_eq!(cell_str(&v, "n"), "3");
        assert_eq!(cell_str(&v, "missing.path"), "-");
    }

    #[test]
    fn cell_str_collapses_multiline_and_truncates() {
        let v = json!({"desc": "line one\nline two\nline three"});
        assert_eq!(cell_str(&v, "desc"), "line one ⏎ line two ⏎ line three");
        let long = "x".repeat(500);
        let v = json!({"big": long});
        let cell = cell_str(&v, "big");
        assert!(cell.ends_with('…'));
        assert!(cell.chars().count() <= MAX_CELL_CHARS + 1);
    }
}
