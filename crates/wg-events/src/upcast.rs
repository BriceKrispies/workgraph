//! Upcasting on read (`core/events/log.py`): a payload written under an older schema version is
//! converted, one version at a time, to the current version of its event type. The current versions
//! are Python's `EVENT_SCHEMA_VERSIONS` (default 1); the steps are its `UPCASTERS` table.

use serde_json::Value;
use wg_model::{JsonObject, StoredEvent};

/// The current schema version of an event type (`EVENT_SCHEMA_VERSIONS`, default 1).
#[must_use]
pub fn current_version(event_type: &str) -> i64 {
    match event_type {
        "task.created" => 2,
        _ => 1,
    }
}

/// One upcaster step: the payload of version `n` to version `n + 1`.
type Step = fn(JsonObject) -> JsonObject;

/// `UPCASTERS`: the step from `(event type, version)`.
fn step(event_type: &str, version: i64) -> Option<Step> {
    match (event_type, version) {
        ("task.created", 1) => Some(task_created_v1_to_v2),
        _ => None,
    }
}

/// `upcast(event_type, version, data)`: applies the steps from `version` up to the current version.
/// A version with no step stays where it is, as Python's loop breaks there.
#[must_use]
pub fn upcast(event_type: &str, version: i64, data: JsonObject) -> (i64, JsonObject) {
    upcast_to(event_type, version, current_version(event_type), data)
}

fn upcast_to(event_type: &str, version: i64, target: i64, data: JsonObject) -> (i64, JsonObject) {
    if version >= target {
        return (version, data);
    }
    match step(event_type, version) {
        Some(apply) => upcast_to(event_type, version + 1, target, apply(data)),
        None => (version, data),
    }
}

/// `to_record`: the stored event with its payload upcast to the current version of its type.
#[must_use]
pub fn read(event: StoredEvent) -> StoredEvent {
    let (schema_version, data) = upcast(&event.event_type, event.schema_version, event.data);
    StoredEvent {
        schema_version,
        data,
        ..event
    }
}

/// `dict.__setitem__` on a JSON object: an existing key keeps its position, a new key goes last.
fn set(object: JsonObject, key: &str, value: Value) -> JsonObject {
    if object.contains_key(key) {
        object
            .into_iter()
            .map(|(name, current)| {
                if name == key {
                    (name, value.clone())
                } else {
                    (name, current)
                }
            })
            .collect()
    } else {
        object
            .into_iter()
            .chain(std::iter::once((key.to_owned(), value)))
            .collect()
    }
}

/// Python `str.strip()`: removes whitespace from both ends. Python's whitespace includes the
/// information separators U+001C to U+001F, which Rust's `char::is_whitespace` does not.
fn py_strip(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `[t.strip() for t in tags.split(",") if t.strip()]`.
fn split_tags(text: &str) -> Value {
    Value::Array(
        text.split(',')
            .map(py_strip)
            .filter(|tag| !tag.is_empty())
            .map(|tag| Value::String(tag.to_owned()))
            .collect(),
    )
}

/// `task.created` v1 to v2. Version 1 stored tags as one comma-separated string and had no
/// acceptance criteria. A missing `tags` reads as the empty string, so it becomes an empty list.
fn task_created_v1_to_v2(data: JsonObject) -> JsonObject {
    let tags_text = match data.get("tags") {
        None => Some(String::new()),
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => None,
    };
    let with_tags = match tags_text {
        Some(text) => set(data, "tags", split_tags(&text)),
        None => data,
    };
    if with_tags.contains_key("acceptance_criteria") {
        with_tags
    } else {
        set(with_tags, "acceptance_criteria", Value::Array(Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object(value: Value) -> JsonObject {
        match value {
            Value::Object(map) => map,
            _ => JsonObject::new(),
        }
    }

    #[test]
    fn v1_tags_split_and_criteria_added() {
        let v1 = object(json!({"key": "WG-1", "tags": " one, ,two ,"}));
        let (version, data) = upcast("task.created", 1, v1);
        assert_eq!(version, 2);
        assert_eq!(data["tags"], json!(["one", "two"]));
        assert_eq!(data["acceptance_criteria"], json!([]));
    }

    #[test]
    fn missing_tags_become_empty_list() {
        let (_, data) = upcast("task.created", 1, object(json!({"key": "WG-2"})));
        assert_eq!(data["tags"], json!([]));
    }

    #[test]
    fn current_version_is_not_upcast_again() {
        let current = object(json!({"key": "WG-3", "tags": "a,b", "acceptance_criteria": ["x"]}));
        let (version, data) = upcast("task.created", 2, current.clone());
        assert_eq!(version, 2);
        assert_eq!(data, current);
    }

    #[test]
    fn other_types_are_version_one() {
        assert_eq!(current_version("task.updated"), 1);
        let (version, _) = upcast("task.updated", 1, JsonObject::new());
        assert_eq!(version, 1);
    }

    #[test]
    fn a_version_without_a_step_stays_put() {
        let (version, _) = upcast("task.created", 0, JsonObject::new());
        assert_eq!(version, 0);
    }
}
