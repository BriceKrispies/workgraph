//! The `git_sync` job's decisions (`jobs/handlers.py`): the repository and mode of a payload, the
//! errors a bad payload or an unknown repository raises, and the JSON object the job returns.

use serde_json::{Value as Json, json};
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
