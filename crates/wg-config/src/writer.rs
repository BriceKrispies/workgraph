//! A TOML writer that reproduces `tomli_w`'s output for the shapes Workgraph writes.
//!
//! Layout: a table's literal keys come first under its header; sub-tables follow depth first in
//! insertion order. A table header is written when the table has literal keys or is empty, so a
//! table that only holds sub-tables has no header of its own. Blocks are separated by one blank
//! line. Non-empty arrays are multi-line with a four-space indent and a trailing comma. Strings are
//! basic strings in which only `"`, `\`, `\b`, `\f`, `\n` and `\r` and the other control characters
//! are escaped (tab stays literal). Floats are written as Python's `repr`.

use serde_json::{Map, Value};

use crate::pyfmt::float_repr;

/// Writes a document as `tomli_w.dumps` would. Callers strip `null` members first with
/// [`crate::strip_none`], as `exclude_none=True` does.
#[must_use]
pub fn render(document: &Map<String, Value>) -> String {
    let mut blocks = Vec::new();
    write_table(document, &[], &mut blocks);
    blocks.join("\n")
}

fn write_table(table: &Map<String, Value>, path: &[&str], blocks: &mut Vec<String>) {
    let literals: Vec<(&String, &Value)> = table
        .iter()
        .filter(|(_, value)| !value.is_object())
        .collect();
    let tables: Vec<(&String, &Map<String, Value>)> = table
        .iter()
        .filter_map(|(key, value)| value.as_object().map(|inner| (key, inner)))
        .collect();

    let has_header = !path.is_empty() && (!literals.is_empty() || table.is_empty());
    if has_header || (path.is_empty() && !literals.is_empty()) {
        let mut block = String::new();
        if has_header {
            block.push('[');
            block.push_str(
                &path
                    .iter()
                    .map(|part| key_text(part))
                    .collect::<Vec<_>>()
                    .join("."),
            );
            block.push_str("]\n");
        }
        for (key, value) in literals {
            block.push_str(&key_text(key));
            block.push_str(" = ");
            block.push_str(&value_text(value));
            block.push('\n');
        }
        blocks.push(block);
    }

    for (key, inner) in tables {
        let mut child_path = path.to_vec();
        child_path.push(key.as_str());
        write_table(inner, &child_path, blocks);
    }
}

/// A key: bare when it is made of ASCII letters, digits, `-` and `_`, else a quoted basic string.
fn key_text(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if bare { key.to_string() } else { quoted(key) }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Null => "\"\"".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => match number.as_i64() {
            Some(int) => int.to_string(),
            None => match number.as_u64() {
                Some(unsigned) => unsigned.to_string(),
                None => float_repr(number.as_f64().unwrap_or(f64::NAN)),
            },
        },
        Value::String(text) => quoted(text),
        Value::Array(items) => {
            if items.is_empty() {
                return "[]".to_string();
            }
            let mut out = String::from("[\n");
            for item in items {
                out.push_str("    ");
                out.push_str(&value_text(item));
                out.push_str(",\n");
            }
            out.push(']');
            out
        }
        Value::Object(members) => {
            let inline: Vec<String> = members
                .iter()
                .map(|(key, inner)| format!("{} = {}", key_text(key), value_text(inner)))
                .collect();
            format!("{{ {} }}", inline.join(", "))
        }
    }
}

fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 && c != '\t' => out.push_str(&format!("\\u{:04x}", c as u32)),
            '\u{7f}' => out.push_str("\\u007f"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
