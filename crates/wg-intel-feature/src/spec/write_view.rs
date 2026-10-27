//! The human lines of the classification writes and of the search writes (`classify request`,
//! `confirm`, `promote`, `reclassify`, `gold`, `search --reindex`), as the Python commands print
//! them. Each takes the reply the service built. The `evaluate` and `search --similar-to` outputs
//! are tables, in [`crate::spec::view`].

use wg_json::{PyStr, Value};

/// A Python string as a Rust string (code points that are not scalar values are dropped).
fn text_of(text: &PyStr) -> String {
    text.code_points()
        .iter()
        .filter_map(|point| char::from_u32(*point))
        .collect()
}

/// The member `name` of a dictionary or model value.
fn member<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Dict(entries) => entries
            .iter()
            .find(|(key, _)| text_of(key) == name)
            .map(|(_, value)| value),
        Value::Model { fields, .. } => fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value),
        _ => None,
    }
}

/// A member as Python's `str()` shows it: text as it is, `None` for null, a float the way Python
/// reprs it (`1.0`, not `1`).
fn shown(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_owned(),
        Some(Value::Str(text)) => text_of(text),
        Some(Value::Float(number)) => format!("{number:?}"),
        Some(Value::Int(number)) => number.to_string(),
        Some(Value::Bool(flag)) => if *flag { "True" } else { "False" }.to_owned(),
        Some(other) => format!("{other:?}"),
    }
}

/// `classify request` (queued): the job id.
#[must_use]
pub fn queued_line(out: &Value) -> String {
    format!("queued classification job {}", shown(member(out, "job_id")))
}

/// `classify request --now` and `classify show`'s row: its status, category, confidence and
/// provider and model.
#[must_use]
pub fn now_line(out: &Value) -> String {
    format!(
        "{}: {} (confidence {}) via {}/{}",
        shown(member(out, "status")),
        shown(member(out, "category")),
        shown(member(out, "confidence")),
        shown(member(out, "provider")),
        shown(member(out, "model")),
    )
}

/// `classify confirm`.
#[must_use]
pub fn confirmed_line(out: &Value) -> String {
    format!("confirmed {}", shown(member(out, "category")))
}

/// `classify promote`.
#[must_use]
pub fn promoted_line(out: &Value) -> String {
    format!("{} is now current", shown(member(out, "category")))
}

/// `classify reclassify`: the batch and the number of jobs queued.
#[must_use]
pub fn reclassified_line(out: &Value) -> String {
    format!(
        "batch {}: {} job(s) queued",
        shown(member(out, "batch_id")),
        shown(member(out, "tasks")),
    )
}

/// `classify gold --category`.
#[must_use]
pub fn gold_added_line(out: &Value) -> String {
    format!("gold label {} added", shown(member(out, "id")))
}

/// `search --reindex`.
#[must_use]
pub fn reindex_line(out: &Value) -> String {
    format!("queued {}", shown(member(out, "enqueued")))
}
