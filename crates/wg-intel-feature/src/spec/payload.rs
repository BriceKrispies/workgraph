//! The readers of a job's payload (`payload.get(...)` in `jobs/handlers.py`): texts, flags and ids,
//! as Python reads them. Pure functions of the payload map.

use serde_json::{Map, Value as Json};
use wg_types::{ErrorCode, Id, WgError};

/// A payload's text, when it is a string.
#[must_use]
pub fn text<'a>(payload: &'a Map<String, Json>, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Json::as_str)
}

/// A payload's truthy flag (`payload.get(key)`, as Python tests it): a missing or null member is
/// false, a boolean is itself, and any other value is true.
#[must_use]
pub fn flag(payload: &Map<String, Json>, key: &str) -> bool {
    match payload.get(key) {
        Some(Json::Bool(value)) => *value,
        Some(Json::Null) | None => false,
        Some(_) => true,
    }
}

/// A payload's id: the string the job was queued with. Anything else is a validation failure.
pub fn id_of(payload: &Map<String, Json>, key: &str) -> Result<Id, WgError> {
    text(payload, key)
        .and_then(|value| value.parse::<Id>().ok())
        .ok_or_else(|| {
            WgError::new(
                ErrorCode::ValidationFailed,
                format!("job payload has no valid {key}"),
            )
        })
}
