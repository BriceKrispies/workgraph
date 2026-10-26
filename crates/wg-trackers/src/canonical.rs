//! Python's `json.dumps(value, sort_keys=True, separators=(",", ":"))` for the values the sync
//! digest hashes: objects, arrays, strings, integers, booleans and null.
//!
//! `ensure_ascii` is on, as in Python's default: every character outside printable ASCII is
//! written as `\uXXXX` (lowercase hex, UTF-16 units for characters beyond the BMP).

use serde_json::Value;

/// The canonical text of `value`, as Python would write it.
///
/// Floats are not written: the digest inputs hold no floats, so one reaching here is a caller bug
/// and is written in Rust's form, which Python's `repr` does not match.
#[must_use]
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(&map[key], out);
            }
            out.push('}');
        }
    }
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            other => {
                let mut units = [0u16; 2];
                for unit in other.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
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
