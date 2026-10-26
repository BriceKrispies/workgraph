//! Python's text of JSON values: `json.dumps(sort_keys=True)` (the dedupe key's canonical form), `str()`
//! and `repr()`, and `float.__repr__`, so that hashes and text match the Python side byte for byte.

use std::collections::BTreeMap;

use serde_json::{Number, Value};

/// `json.dumps(value, sort_keys=True)` with Python's default separators and `ensure_ascii`.
pub fn dumps_sorted(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(true) => "true".to_owned(),
        Value::Bool(false) => "false".to_owned(),
        Value::Number(number) => json_number(number),
        Value::String(text) => ascii_string(text),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(dumps_sorted)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            format!(
                "{{{}}}",
                sorted
                    .iter()
                    .map(|(key, item)| format!("{}: {}", ascii_string(key), dumps_sorted(item)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

/// Python `str(value)`: a string is itself, other values print as their `repr`.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => py_repr(other),
    }
}

/// Python `repr(value)` of a value decoded from JSON.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(number) => python_number(number),
        Value::String(text) => wg_model::py_repr_str(text),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(key, item)| format!("{}: {}", wg_model::py_repr_str(key), py_repr(item)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python truthiness of an optional JSON value (`None`, `null`, `0`, `""`, `[]`, `{}` and `false` are falsy).
pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|x| x != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(map)) => !map.is_empty(),
    }
}

/// `float.__repr__`: the shortest digits that round-trip, fixed notation for exponents in `-4..16`.
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let sign = if mantissa.starts_with('-') { "-" } else { "" };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let body = if (-4..16).contains(&exponent) {
        fixed_notation(&digits, exponent)
    } else {
        let (first, rest) = digits.split_at(1);
        let mantissa = if rest.is_empty() {
            first.to_owned()
        } else {
            format!("{first}.{rest}")
        };
        let exponent_sign = if exponent < 0 { "-" } else { "+" };
        format!("{mantissa}e{exponent_sign}{:02}", exponent.abs())
    };
    format!("{sign}{body}")
}

fn fixed_notation(digits: &str, exponent: i32) -> String {
    if exponent >= 0 {
        let integer_len = exponent as usize + 1;
        let padded = format!(
            "{digits}{}",
            "0".repeat(integer_len.saturating_sub(digits.len()))
        );
        let (integer, fraction) = padded.split_at(integer_len);
        let fraction = if fraction.is_empty() { "0" } else { fraction };
        format!("{integer}.{fraction}")
    } else {
        let zeros = "0".repeat((-exponent - 1) as usize);
        format!("0.{zeros}{digits}")
    }
}

fn python_number(number: &Number) -> String {
    if number.is_f64() {
        number
            .as_f64()
            .map_or_else(|| number.to_string(), float_repr)
    } else {
        number.to_string()
    }
}

/// `json.dumps` text of a number: floats use `float.__repr__`, non-finite values use the JSON spellings.
fn json_number(number: &Number) -> String {
    match number.as_f64() {
        Some(x) if number.is_f64() && x.is_nan() => "NaN".to_owned(),
        Some(x) if number.is_f64() && x.is_infinite() => {
            if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
        }
        _ => python_number(number),
    }
}

/// A JSON string with `ensure_ascii`: printable ASCII as is, everything else as `\uXXXX` (surrogate pairs
/// above the BMP).
pub fn ascii_string(text: &str) -> String {
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
            ' '..='~' => c.to_string(),
            other => escape_code_point(u32::from(other)),
        })
        .collect();
    format!("\"{body}\"")
}

fn escape_code_point(code: u32) -> String {
    if code < 0x1_0000 {
        format!("\\u{code:04x}")
    } else {
        let offset = code - 0x1_0000;
        let high = 0xD800 + (offset >> 10);
        let low = 0xDC00 + (offset & 0x3FF);
        format!("\\u{high:04x}\\u{low:04x}")
    }
}
