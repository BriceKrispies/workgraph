//! `config set KEY VALUE`: the value parsing and the dotted-path update.

use serde_json::{Map, Value};

/// The value of `config set`: parsed as JSON when it is valid JSON, else the text itself.
#[must_use]
pub fn parse_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

/// The document with `value` stored at the dotted `key`, creating missing tables on the way.
///
/// Returns `None` when a segment of the path before the last one is not a table (a scalar, or a
/// `null`), which `config set` reports as a key that is not nested.
#[must_use]
pub fn set_path(base: &Map<String, Value>, key: &str, value: Value) -> Option<Map<String, Value>> {
    let mut target = base.clone();
    let parts: Vec<&str> = key.split('.').collect();
    let (last, parents) = parts.split_last()?;
    let mut cursor = &mut target;
    for part in parents {
        let entry = cursor
            .entry((*part).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        cursor = entry.as_object_mut()?;
    }
    cursor.insert((*last).to_string(), value);
    Some(target)
}
