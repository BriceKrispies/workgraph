//! Workspace settings: the accepted keys, their validation and the merge that stores them.
//!
//! The rules of `WorkspaceService.update_settings` in `core/services/workspaces.py`. The shell
//! loads the workspace, calls [`validate_update`] and [`merge`], and writes the result together
//! with the `workspace.settings_changed` event, whose data is the `before` and `after` objects.

use std::collections::BTreeSet;

use serde_json::Value;
use wg_model::{JsonObject, py_repr_str};
use wg_types::{ErrorCode, WgError};

/// The setting keys a workspace may store (Python's `ALLOWED_SETTINGS`).
pub const ALLOWED_SETTINGS: [&str; 3] = [
    "allowed_roots",
    "default_project",
    "description_classification",
];

/// Checks an update before it is merged: every key must be an allowed setting (the unknown ones
/// are listed sorted, as Python's `sorted(set)` prints them), and `allowed_roots`, when present,
/// must be a list of strings.
pub fn validate_update(update: &JsonObject) -> Result<(), WgError> {
    let unknown: BTreeSet<&str> = update
        .keys()
        .map(String::as_str)
        .filter(|key| !ALLOWED_SETTINGS.contains(key))
        .collect();
    if !unknown.is_empty() {
        let listed: Vec<String> = unknown.iter().map(|key| py_repr_str(key)).collect();
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("unknown workspace settings: [{}]", listed.join(", ")),
        ));
    }
    if let Some(roots) = update.get("allowed_roots") {
        let valid = roots
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string));
        if !valid {
            return Err(WgError::new(
                ErrorCode::ValidationFailed,
                "allowed_roots must be a list of directory paths",
            ));
        }
    }
    Ok(())
}

/// The stored settings after an update: the update's keys overwrite the existing ones and the
/// other keys are kept (Python's `{**before, **settings}`).
pub fn merge(before: &JsonObject, update: &JsonObject) -> JsonObject {
    // Existing keys keep their position and take the update's value; new keys follow in the
    // update's order, as an insert loop over a clone would leave them.
    before
        .iter()
        .map(|(key, value)| (key.clone(), update.get(key).unwrap_or(value).clone()))
        .chain(
            update
                .iter()
                .filter(|(key, _)| !before.contains_key(*key))
                .map(|(key, value)| (key.clone(), value.clone())),
        )
        .collect()
}
