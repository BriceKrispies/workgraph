//! The `git_sync` job's decisions (`jobs/handlers.py`): the repository and mode of a payload, the
//! errors a bad payload or an unknown repository raises, and the JSON object the job returns.

use serde_json::{Value as Json, json};
use wg_json::Value as WireValue;
use wg_model::entity::JsonObject;
use wg_types::{Id, WgError, parse_id};

use super::ingest::SyncResult;

/// The message of the bare `KeyError` a job without `repository_id` raises (retried, kind
/// `exception`, as Python's worker treats any unexpected exception).
pub const MISSING_REPOSITORY: &str = "KeyError: 'repository_id'";

/// The `repository_id` of a job payload, when it is text.
pub fn repository_reference(payload: &JsonObject) -> Option<&str> {
    payload.get("repository_id").and_then(Json::as_str)
}

/// `mode` of the payload: `incremental` when absent, `full` or `incremental` otherwise. Any other
/// value is a `ValidationFailed`, which Python raises before it looks the repository up.
pub fn full_mode(mode: Option<&Json>) -> Result<bool, WgError> {
    match mode {
        None => Ok(false),
        Some(Json::String(text)) if text == "full" => Ok(true),
        Some(Json::String(text)) if text == "incremental" => Ok(false),
        Some(_) => Err(WgError::validation("mode must be incremental or full")),
    }
}

/// The id of the repository a reference names. A reference that is not an id names no repository,
/// as Python's `session.get` finds none.
pub fn repository_of(reference: &str) -> Result<Id, WgError> {
    parse_id(reference).map_err(|_| unknown_repository(reference))
}

/// The NotFound of an unknown repository, worded as `GitService._repo_info` words it.
pub fn unknown_repository(reference: &str) -> WgError {
    WgError::not_found(format!("repository {reference} not found"))
}

/// The job's result: the `SyncResult` dict (`model_dump()`), fields in the Python class's order.
pub fn sync_result_object(result: &SyncResult) -> JsonObject {
    json!({
        "repository_id": result.repository_id,
        "repository": result.repository,
        "mode": result.mode,
        "new_commits": result.new_commits,
        "refs_changed": result.refs_changed,
        "marked_unreachable": result.marked_unreachable,
        "restored_reachable": result.restored_reachable,
        "equivalences": result.equivalences,
        "links_created": result.links_created,
        "shallow": result.shallow,
        "head_refs": result.head_refs,
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

/// The message of the bare `KeyError` a `compute_change_surface` job without `task_id` raises
/// (retried, kind `exception`, as Python's worker treats any unexpected exception).
pub const MISSING_TASK: &str = "KeyError: 'task_id'";

/// The `task_id` of a `compute_change_surface` payload, when it is text.
pub fn task_reference(payload: &JsonObject) -> Option<&str> {
    payload.get("task_id").and_then(Json::as_str)
}

/// The number of repository surfaces `git surface` replied with (`len(surfaces)` in Python).
pub fn surface_count(replies: &WireValue) -> usize {
    match replies {
        WireValue::List(items) => items.len(),
        _ => 0,
    }
}

/// The job's result of `compute_change_surface`: `{"repositories": n}`.
pub fn surface_result_object(repositories: usize) -> JsonObject {
    json!({ "repositories": repositories })
        .as_object()
        .cloned()
        .unwrap_or_default()
}

/// The message of the bare `KeyError` an `analyze_commits` job without `commit_ids` raises.
pub const MISSING_COMMIT_IDS: &str = "KeyError: 'commit_ids'";

/// The commits of an `analyze_commits` payload. An entry that is not an id names no commit, which
/// Python's lookup finds none of, so it is skipped. `None` when `commit_ids` is absent or not a list.
pub fn commit_ids(payload: &JsonObject) -> Option<Vec<Id>> {
    let items = payload.get("commit_ids").and_then(Json::as_array)?;
    Some(
        items
            .iter()
            .filter_map(Json::as_str)
            .filter_map(|text| parse_id(text).ok())
            .collect(),
    )
}

/// The job's result of `analyze_commits`: `{"analyzed": n}`.
pub fn analyze_result_object(analyzed: usize) -> JsonObject {
    json!({ "analyzed": analyzed })
        .as_object()
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_ids_skip_names_that_are_not_ids() {
        let payload = json!({ "commit_ids": ["nope", 3] })
            .as_object()
            .cloned()
            .unwrap_or_default();
        assert_eq!(commit_ids(&payload), Some(Vec::new()));
        assert_eq!(commit_ids(&JsonObject::new()), None);
        assert_eq!(analyze_result_object(2).get("analyzed"), Some(&json!(2)));
    }

    #[test]
    fn surface_result_counts_repositories() {
        let replies = WireValue::List(vec![WireValue::Null, WireValue::Null]);
        assert_eq!(surface_count(&replies), 2);
        assert_eq!(surface_count(&WireValue::Null), 0);
        let object = surface_result_object(surface_count(&replies));
        assert_eq!(object.get("repositories"), Some(&json!(2)));
    }
}
