//! Tracker ownership of tracked tasks: the fields a tracker owns, the guard that rejects other
//! actors' edits to them, and the rules for naming a task after its tracker issue. Mirrors
//! `TRACKER_OWNED_FIELDS`, `_guard_tracker_owned` and `_check_tracker_key` in
//! `core/services/tasks.py`.

use std::collections::BTreeSet;

use serde_json::json;
use wg_model::TaskSlice;
use wg_model::entity::{ExternalItem, Task};
use wg_model::stamp::py_repr_str;
use wg_types::{Actor, ErrorCode, Id, WgError};

/// The actor string of the tracker sync (`SYNC_ACTOR`).
pub const SYNC_ACTOR: &str = "system:tracker-sync";

/// Fields a tracker owns on a tracked task. Only the sync may change them. `status` is guarded
/// only for changes that accept, cancel or reopen the task.
pub const TRACKER_OWNED_FIELDS: &[&str] = &[
    "title",
    "description",
    "acceptance_criteria",
    "priority",
    "assignee",
    "parent",
    "archived",
    "status",
];

/// Whether `actor` is the tracker sync.
pub fn is_sync_actor(actor: &Actor) -> bool {
    actor.to_string() == SYNC_ACTOR
}

/// The tracker issue a task mirrors, if any (at most one `external_items` row per task).
pub fn tracked_by(tasks: &TaskSlice, task_id: Id) -> Option<&ExternalItem> {
    tasks
        .external_items
        .values()
        .find(|item| item.task_id == task_id)
}

/// Rejects a change to tracker-owned `fields` of a tracked task unless the actor is the sync.
/// `fields` are the names the change touches (`parent` for a parent change, `status` for a
/// status change that accepts, cancels or reopens). Fields the tracker does not own pass.
pub fn guard_tracker_owned(
    actor: &Actor,
    task: &Task,
    fields: &BTreeSet<&str>,
    item: Option<&ExternalItem>,
) -> Result<(), WgError> {
    if is_sync_actor(actor) {
        return Ok(());
    }
    let owned: Vec<&str> = fields
        .iter()
        .copied()
        .filter(|field| TRACKER_OWNED_FIELDS.contains(field))
        .collect();
    let Some(item) = item else {
        return Ok(());
    };
    if owned.is_empty() {
        return Ok(());
    }
    Err(WgError::new(
        ErrorCode::PermissionDenied,
        format!(
            "{} mirrors {} in {}; the tracker owns {} for it, so change them there",
            task.key,
            item.external_key,
            item.connection,
            owned.join(", ")
        ),
    )
    .with_detail("fields", json!(owned))
    .with_detail("connection", json!(item.connection))
    .with_detail("external_key", json!(item.external_key)))
}

/// Whether `key` looks like a tracker issue key: an uppercase letter, up to nine more uppercase
/// letters or digits, a dash, then digits (`TRACKER_KEY_RE`).
pub fn is_tracker_key(key: &str) -> bool {
    let Some((prefix, number)) = key.split_once('-') else {
        return false;
    };
    let mut chars = prefix.chars();
    let prefix_ok = matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
        && prefix.len() <= 10
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    prefix_ok && !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
}

/// The checks of naming a new task after its tracker issue. Only the sync may do it; the key
/// must look like `ABC-123`, must not use the workspace's own prefix, and must not be taken
/// (`key_taken`).
pub fn check_tracker_key(
    actor: &Actor,
    key: &str,
    workspace_prefix: &str,
    key_taken: bool,
) -> Result<(), WgError> {
    if !is_sync_actor(actor) {
        return Err(WgError::new(
            ErrorCode::PermissionDenied,
            format!("only {SYNC_ACTOR} may set a task key"),
        ));
    }
    if !is_tracker_key(key) {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("tracker key {} must look like ABC-123", py_repr_str(key)),
        ));
    }
    let prefix = key.split_once('-').map_or(key, |(prefix, _)| prefix);
    if prefix == workspace_prefix {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("tracker key prefix {prefix} is the workspace's own prefix"),
        ));
    }
    if key_taken {
        return Err(WgError::new(
            ErrorCode::Conflict,
            format!("task {key} already exists in this workspace"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_keys_match_the_python_pattern() {
        assert!(is_tracker_key("ABC-123"));
        assert!(is_tracker_key("A1-7"));
        assert!(!is_tracker_key("abc-1"));
        assert!(!is_tracker_key("ABC-"));
        assert!(!is_tracker_key("ABC123"));
        assert!(!is_tracker_key("ABCDEFGHIJK-1"));
        assert!(!is_tracker_key("1ABC-1"));
    }
}
