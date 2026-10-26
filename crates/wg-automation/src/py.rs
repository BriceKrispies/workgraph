//! Python value semantics that automation output depends on: `repr` and `str` of JSON values,
//! `==`, ordering, truthiness and the `in` operator, including the cases where Python raises
//! `TypeError` (modelled as `None`, which the callers turn into `False`).

use std::cmp::Ordering;

use serde_json::Value;

/// Python's `repr()` of a float: shortest round-trip digits, `.0` for whole numbers, and
/// exponent form outside `1e-4 <= |x| < 1e16`.
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let sign = if mantissa.starts_with('-') { "-" } else { "" };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let body = if (-4..16).contains(&exponent) {
        if exponent >= 0 {
            let int_len = exponent as usize + 1;
            let int_part = if digits.len() >= int_len {
                digits[..int_len].to_owned()
            } else {
                format!("{digits}{}", "0".repeat(int_len - digits.len()))
            };
            let frac = if digits.len() > int_len {
                &digits[int_len..]
            } else {
                "0"
            };
            format!("{int_part}.{frac}")
        } else {
            format!("0.{}{digits}", "0".repeat((-exponent - 1) as usize))
        }
    } else {
        let (head, tail) = digits.split_at(1);
        let frac = if tail.is_empty() {
            String::new()
        } else {
            format!(".{tail}")
        };
        let exp_sign = if exponent < 0 { '-' } else { '+' };
        format!("{head}{frac}e{exp_sign}{:02}", exponent.abs())
    };
    format!("{sign}{body}")
}

/// Python's `repr()` of a string: single quotes unless the text holds a single quote and no
/// double quote, with backslash and control-character escapes.
pub fn str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let body: String = text
        .chars()
        .map(|ch| match ch {
            '\\' => "\\\\".to_owned(),
            '\n' => "\\n".to_owned(),
            '\r' => "\\r".to_owned(),
            '\t' => "\\t".to_owned(),
            c if c == quote => format!("\\{c}"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => format!("\\x{:02x}", c as u32),
            c => c.to_string(),
        })
        .collect();
    format!("{quote}{body}{quote}")
}

/// Python's `repr()` of a JSON-shaped value.
pub fn repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(number) => match (number.as_i64(), number.as_u64()) {
            (Some(int), _) => int.to_string(),
            (None, Some(int)) => int.to_string(),
            (None, None) => float_repr(number.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(text) => str_repr(text),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(repr).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", str_repr(k), repr(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// Python's `str()`: strings are returned as they are, everything else is `repr`.
pub fn str_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => repr(other),
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        Value::Number(number) => number.as_f64(),
        _ => None,
    }
}

/// Python `==`: numbers and booleans compare by value (`1 == 1.0 == True`), containers
/// element-wise.
pub fn eq(left: &Value, right: &Value) -> bool {
    if let (Some(a), Some(b)) = (number(left), number(right)) {
        return a == b;
    }
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| eq(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| eq(value, other)))
        }
        _ => false,
    }
}

/// Python `<`/`>` ordering. `None` is the `TypeError` case (incomparable operands).
pub fn compare(left: &Value, right: &Value) -> Option<Ordering> {
    if let (Some(a), Some(b)) = (number(left), number(right)) {
        return a.partial_cmp(&b);
    }
    match (left, right) {
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Array(a), Value::Array(b)) => {
            for (x, y) in a.iter().zip(b) {
                if eq(x, y) {
                    continue;
                }
                return compare(x, y);
            }
            Some(a.len().cmp(&b.len()))
        }
        _ => None,
    }
}

/// Python truthiness (`bool(value)`).
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// Python `item in container`. `None` means the container is not iterable or the item is not
/// hashable for it (`TypeError`).
pub fn contains(container: &Value, item: &Value) -> Option<bool> {
    match container {
        Value::Array(items) => Some(items.iter().any(|candidate| eq(item, candidate))),
        Value::String(text) => match item {
            Value::String(needle) => Some(text.contains(needle.as_str())),
            _ => None,
        },
        Value::Object(map) => match item {
            Value::String(key) => Some(map.contains_key(key)),
            Value::Null | Value::Bool(_) | Value::Number(_) => Some(false),
            _ => None,
        },
        _ => None,
    }
}
