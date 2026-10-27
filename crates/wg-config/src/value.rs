//! Helpers over JSON-shaped configuration values: Python's truthiness, `exclude_none`, and the
//! deep merge used for workspace overrides.

use serde_json::{Map, Value};

/// The document without its `null` members, at every depth (`model_dump(exclude_none=True)`).
#[must_use]
pub fn strip_none(map: &Map<String, Value>) -> Map<String, Value> {
    map.iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| (key.clone(), strip_value(value)))
        .collect()
}

fn strip_value(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(strip_none(members)),
        Value::Array(items) => Value::Array(items.iter().map(strip_value).collect()),
        other => other.clone(),
    }
}

/// `_deep_merge`: nested objects merge key by key, every other value of `over` replaces `base`.
pub(crate) fn deep_merge(
    base: &Map<String, Value>,
    over: &Map<String, Value>,
) -> Map<String, Value> {
    let mut merged = base.clone();
    for (key, value) in over {
        let combined = match (merged.get(key), value) {
            (Some(Value::Object(inner)), Value::Object(incoming)) => {
                Value::Object(deep_merge(inner, incoming))
            }
            _ => value.clone(),
        };
        merged.insert(key.clone(), combined);
    }
    merged
}
