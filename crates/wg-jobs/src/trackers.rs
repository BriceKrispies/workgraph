//! Enqueueing tracker syncs (`jobs/trackers.py`): at most one queued or running sync per connection
//! and mode.

use serde_json::Value;
use wg_model::decision::Decision;
use wg_model::entity::JsonObject;
use wg_model::py_repr_str;
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_types::{Id, WgError};

use crate::limits::tracker_key;
use crate::queue::{ACTIVE_STATUSES, NewJob, enqueue};

/// The job kind of a tracker sync.
pub const TRACKER_SYNC_KIND: &str = "tracker_sync";

/// Queues a sync of `connection` and replies with its job id. A queued or running sync of the same
/// connection and mode is returned instead of queued again. The idempotency key includes a fresh
/// id, so a later sync never maps onto an earlier, finished job.
///
/// `known_connections` lists the configured connections; an unknown one is `not_found`.
pub fn enqueue_tracker_sync(
    slice: &QueueSlice,
    connection: &str,
    full: bool,
    known_connections: &[&str],
    created_by: &str,
    stamp: &Stamp,
) -> Result<Decision<Id>, WgError> {
    if !known_connections.contains(&connection) {
        return Err(WgError::not_found(format!(
            "tracker connection {} is not configured",
            py_repr_str(connection)
        ))
        .detail("connection", Value::String(connection.to_owned())));
    }
    let active = slice.jobs.values().find(|job| {
        job.kind == TRACKER_SYNC_KIND
            && ACTIVE_STATUSES.contains(&job.status)
            && job.payload.get("connection").and_then(Value::as_str) == Some(connection)
            && payload_is_full(&job.payload) == full
    });
    match active {
        Some(job) => Ok(Decision::new(job.id)),
        None => {
            let mode = if full { "full" } else { "incremental" };
            let payload: JsonObject = [
                (
                    "connection".to_owned(),
                    Value::String(connection.to_owned()),
                ),
                ("full".to_owned(), Value::Bool(full)),
            ]
            .into_iter()
            .collect();
            let job = NewJob {
                payload,
                resource_keys: vec![tracker_key(connection)],
                idempotency_key: Some(format!("tracker-sync:{connection}:{mode}:{}", stamp.id(1))),
                created_by: created_by.to_owned(),
                ..NewJob::new(TRACKER_SYNC_KIND)
            };
            Ok(enqueue(slice, &job, stamp))
        }
    }
}

/// `bool(payload.get("full"))` of the Python check.
fn payload_is_full(payload: &JsonObject) -> bool {
    payload
        .get("full")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}
