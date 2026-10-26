//! Python's `json.dumps(value, sort_keys=True, separators=(",", ":"))` for the values the sync
//! digest hashes: objects, arrays, strings, integers, booleans and null.
//!
//! `ensure_ascii` is on, as in Python's default: every character outside printable ASCII is
//! written as `\uXXXX` (lowercase hex, UTF-16 units for characters beyond the BMP).

use std::collections::BTreeMap;

use serde_json::Value;

/// The canonical text of `value`, as Python would write it.
///
/// Floats are not written: the digest inputs hold no floats, so one reaching here is a caller bug
/// and is written in Rust's form, which Python's `repr` does not match.
#[must_use]
pub fn canonical_json(value: &Value) -> String {
    canonical_value(value)
}

fn canonical_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(true) => "true".to_string(),
        Value::Bool(false) => "false".to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => quoted(text),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canonical_value).collect();
            format!("[{}]", items.join(","))
        }
        Value::Object(map) => {
            // Keys sorted by their string order, as Python's `sort_keys=True` does.
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            let members: Vec<String> = sorted
                .iter()
                .map(|(key, value)| format!("{}:{}", quoted(key), canonical_value(value)))
                .collect();
            format!("{{{}}}", members.join(","))
        }
    }
}

/// `text` as a JSON string literal, with every character escaped as Python does.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.chars().map(escape_char).collect::<String>())
}

fn escape_char(c: char) -> String {
    match c {
        '"' => "\\\"".to_string(),
        '\\' => "\\\\".to_string(),
        '\n' => "\\n".to_string(),
        '\r' => "\\r".to_string(),
        '\t' => "\\t".to_string(),
        '\u{8}' => "\\b".to_string(),
        '\u{c}' => "\\f".to_string(),
        ' '..='~' => c.to_string(),
        other => other
            .to_string()
            .encode_utf16()
            .map(|unit| format!("\\u{unit:04x}"))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::canonical_json;

    #[test]
    fn keys_are_sorted_and_separators_are_compact() {
        assert_eq!(
            canonical_json(&json!({"b": [1, null], "a": {"y": true, "x": false}})),
            r#"{"a":{"x":false,"y":true},"b":[1,null]}"#
        );
    }

    #[test]
    fn non_ascii_is_escaped_as_python_does() {
        assert_eq!(
            canonical_json(&json!("r-\u{e9}\u{2603}")),
            format!("\"r-\\u{:04x}\\u{:04x}\"", 0xe9, 0x2603)
        );
        assert_eq!(
            canonical_json(&json!("\u{1F389}")),
            format!("\"\\u{:04x}\\u{:04x}\"", 0xd83c, 0xdf89)
        );
        assert_eq!(
            canonical_json(&json!("a\"\\\n\u{1}\u{7f}")),
            r#""a\"\\\n\u0001\u007f""#
        );
    }
}
