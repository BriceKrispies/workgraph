//! Python-compatible helpers: truthiness, `str()`, character slicing and `json.dumps`.

use serde_json::Value;

/// Python `str(value)` for a decoded JSON value. Lists and dicts use their JSON form, which
/// differs from Python's `repr` only in quoting; the adapters only ever see strings there.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::String(s) => s.clone(),
        other => py_dumps(other, None),
    }
}

/// A JSON integer (not a float): Python `isinstance(x, int)`.
pub fn as_int(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) if n.is_i64() || n.is_u64() => n.as_i64(),
        _ => None,
    }
}

/// A JSON number as a float: Python `isinstance(x, int | float)`.
pub fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// Python `json.dumps(value)` (`indent=None`, separators `", "` and `": "`) or
/// `json.dumps(value, indent=n)`, both with `ensure_ascii=True`.
pub fn py_dumps(value: &Value, indent: Option<usize>) -> String {
    dump(value, indent, 0)
}

fn dump(value: &Value, indent: Option<usize>, depth: usize) -> String {
    match value {
        Value::String(s) => quote(s),
        Value::Array(items) if !items.is_empty() => {
            let parts = items.iter().map(|v| dump(v, indent, depth + 1));
            wrap(('[', ']'), parts.collect(), indent, depth)
        }
        Value::Object(map) if !map.is_empty() => {
            let parts = map
                .iter()
                .map(|(k, v)| format!("{}: {}", quote(k), dump(v, indent, depth + 1)));
            wrap(('{', '}'), parts.collect(), indent, depth)
        }
        Value::Array(_) => "[]".to_owned(),
        Value::Object(_) => "{}".to_owned(),
        other => other.to_string(),
    }
}

fn wrap(
    (open, close): (char, char),
    parts: Vec<String>,
    indent: Option<usize>,
    depth: usize,
) -> String {
    match indent {
        None => format!("{open}{}{close}", parts.join(", ")),
        Some(n) => {
            let inner = " ".repeat(n * (depth + 1));
            let outer = " ".repeat(n * depth);
            let body = parts
                .iter()
                .map(|p| format!("{inner}{p}"))
                .collect::<Vec<_>>()
                .join(",\n");
            format!("{open}\n{body}\n{outer}{close}")
        }
    }
}

fn quote(text: &str) -> String {
    let body: String = text
        .chars()
        .map(|c| match c {
            '"' => "\\\"".to_owned(),
            '\\' => "\\\\".to_owned(),
            '\n' => "\\n".to_owned(),
            '\r' => "\\r".to_owned(),
            '\t' => "\\t".to_owned(),
            '\u{8}' => "\\b".to_owned(),
            '\u{c}' => "\\f".to_owned(),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut units = [0u16; 2];
                c.encode_utf16(&mut units)
                    .iter()
                    .map(|u| format!("\\u{u:04x}"))
                    .collect()
            }
            c => c.to_string(),
        })
        .collect();
    format!("\"{body}\"")
}

/// A float as Python `str(float)`.
pub fn py_float(value: f64) -> String {
    wg_json::py_float_repr(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dumps_matches_python() {
        let v = json!({"a": [1, 2], "b": {}, "c": "é\n😀", "d": []});
        assert_eq!(
            py_dumps(&v, None),
            "{\"a\": [1, 2], \"b\": {}, \"c\": \"\\u00e9\\n\\ud83d\\ude00\", \"d\": []}"
        );
        assert_eq!(
            py_dumps(&json!({"a": [1], "b": {"c": null}}), Some(2)),
            "{\n  \"a\": [\n    1\n  ],\n  \"b\": {\n    \"c\": null\n  }\n}"
        );
    }

    #[test]
    fn python_scalars() {
        assert_eq!(py_str(&Value::Null), "None");
        assert_eq!(as_int(&json!(1.5)), None);
        assert_eq!(as_int(&json!(3)), Some(3));
    }
}
