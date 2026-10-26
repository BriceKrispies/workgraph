//! [`StoredEvent`]: one row of the `events` log as it is read back, `seq` included.
//!
//! An [`EventDraft`](super::EventDraft) is what a decision produces; a stored event is what the
//! reader gets. The two differ in two ways that show on the wire: the row has the `seq` SQLite
//! assigned, and the `data` column is stored with sorted keys (`json.dumps(sort_keys=True)`), so
//! it reads back sorted at every depth, not in the order the payload struct lists them.

use serde_json::Value;
use wg_types::{Correlation, Id, Timestamp, WgError};

use super::EventDraft;
use crate::entity::JsonObject;

/// One `events` row.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredEvent {
    pub seq: i64,
    pub id: Id,
    pub workspace_id: Option<Id>,
    pub event_type: String,
    pub entity_type: String,
    pub entity_id: Id,
    pub actor: String,
    pub actor_kind: String,
    pub occurred_at: Timestamp,
    pub correlation_id: Correlation,
    pub causation_id: Option<Id>,
    pub attempt_id: Option<Id>,
    pub schema_version: i64,
    /// The `data` column, keys sorted at every depth.
    pub data: JsonObject,
}

impl StoredEvent {
    /// The row the store would hold for `draft` appended with sequence number `seq`.
    pub fn from_draft(seq: i64, draft: &EventDraft) -> Result<Self, WgError> {
        Ok(StoredEvent {
            seq,
            id: draft.id,
            workspace_id: draft.workspace_id,
            event_type: draft.event_type().to_owned(),
            entity_type: draft.entity_type().as_str().to_owned(),
            entity_id: draft.entity_id,
            actor: draft.actor.to_string(),
            actor_kind: draft.actor.kind().as_str().to_owned(),
            occurred_at: draft.occurred_at,
            correlation_id: draft.correlation_id.clone(),
            causation_id: draft.causation_id,
            attempt_id: draft.attempt_id,
            schema_version: i64::from(draft.schema_version()),
            data: sort_object(&draft.data()?),
        })
    }
}

/// `json.dumps(value, sort_keys=True)` and back: object keys in code point order at every depth.
/// UTF-8 byte order of Rust strings equals code point order.
pub fn sort_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(sort_object(map)),
        Value::Array(items) => Value::Array(items.iter().map(sort_keys).collect()),
        other => other.clone(),
    }
}

fn sort_object(map: &JsonObject) -> JsonObject {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
        .into_iter()
        .map(|(key, value)| (key.clone(), sort_keys(value)))
        .collect()
}
