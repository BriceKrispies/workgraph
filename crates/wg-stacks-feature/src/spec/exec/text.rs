//! `json.dumps(value, sort_keys=True)` as the executor stores item details and landing requests:
//! Python's default separators (`", "` and `": "`) and ASCII-only output.

use serde_json::Value;

/// The text `json.dumps(value, sort_keys=True)` gives.
#[must_use]
pub fn dumps_sorted(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => quote(text),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(dumps_sorted)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<&String, &Value> = map.iter().collect();
            format!(
                "{{{}}}",
                sorted
                    .into_iter()
                    .map(|(key, member)| format!("{}: {}", quote(key), dumps_sorted(member)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

/// A JSON string with `ensure_ascii`: non-ASCII characters become `\uXXXX` (surrogate pairs
/// above the basic plane).
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
            c if (' '..='~').contains(&c) => c.to_string(),
            c => {
                let mut units = [0_u16; 2];
                c.encode_utf16(&mut units)
                    .iter()
                    .map(|unit| format!("\\u{unit:04x}"))
                    .collect()
            }
        })
        .collect();
    format!("\"{body}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_are_sorted_and_separators_are_pythons() {
        let value = json!({"b": [1, 2], "a": {"y": null, "x": true}});
        assert_eq!(
            dumps_sorted(&value),
            r#"{"a": {"x": true, "y": null}, "b": [1, 2]}"#
        );
    }

    #[test]
    fn non_ascii_is_escaped() {
        let escaped = format!("\"{bs}u00e9{bs}ud83d{bs}ude00\"", bs = '\\');
        assert_eq!(dumps_sorted(&json!("é😀")), escaped);
    }
}
