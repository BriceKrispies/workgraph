//! The Python field validators of `core/models/schemas.py`.

use regex::Regex;
use serde_json::Value;

use crate::schema::Check;

/// Python's `repr` of a `str`.
fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn matches(pattern: &str, text: &str) -> bool {
    Regex::new(pattern).is_ok_and(|re| re.is_match(text))
}

const SLUG: &str = "^[a-z0-9][a-z0-9-]{0,62}$";
const KEY_PREFIX: &str = "^[A-Z][A-Z0-9]{0,9}$";
const TAG: &str = "^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,99}$";

/// Runs `check` on an already validated value. The error is the `ValueError` message. A null
/// passes unchanged, as the Python validators guard on `None`.
pub(crate) fn run(check: Check, value: &Value) -> Result<Value, String> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    match (check, value) {
        (Check::Slug, Value::String(text)) => {
            if matches(SLUG, text) {
                Ok(value.clone())
            } else {
                Err("slug must be lowercase letters, digits and dashes".to_string())
            }
        }
        (Check::KeyPrefix, Value::String(text)) => {
            let upper = text.to_uppercase();
            if matches(KEY_PREFIX, &upper) {
                Ok(Value::String(upper))
            } else {
                Err("key prefix must be alphanumeric and start with a letter".to_string())
            }
        }
        (Check::Tags, Value::Array(items)) => {
            let mut cleaned: Vec<Value> = Vec::new();
            for item in items {
                let tag = item.as_str().unwrap_or_default().trim();
                if !matches(TAG, tag) {
                    return Err(format!("invalid tag {}", py_repr(tag)));
                }
                let tag = Value::String(tag.to_string());
                if !cleaned.contains(&tag) {
                    cleaned.push(tag);
                }
            }
            Ok(Value::Array(cleaned))
        }
        (Check::Files, Value::Array(items)) => {
            let mut cleaned: Vec<Value> = Vec::new();
            for item in items {
                let key = (item.get("path"), item.get("anchor"));
                if !cleaned
                    .iter()
                    .any(|seen| (seen.get("path"), seen.get("anchor")) == key)
                {
                    cleaned.push(item.clone());
                }
            }
            Ok(Value::Array(cleaned))
        }
        (Check::EmptyToNone, Value::String(text)) if text.is_empty() => Ok(Value::Null),
        _ => Ok(value.clone()),
    }
}
