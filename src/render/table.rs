use serde_json::Value;
use tabled::builder::Builder;
use tabled::settings::Style;

use crate::error::Error;

/// Render a list of items as a table or raw JSON.
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

pub fn print_json(v: &Value) -> Result<(), Error> {
    println!(
        "{}",
        serde_json::to_string_pretty(v).map_err(|e| Error::runtime(format!("json: {e}")))?
    );
    Ok(())
}

fn cell_str(item: &Value, key: &str) -> String {
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
}
