//! `EventOut`: one row of the `events` table, as the event list and the SSE stream print it.

use wg_json::Value;
use wg_model::event::EventDraft;
use wg_types::{Id, WgError};

use crate::convention::{TimeFormat, id, json_object, model, text, timestamp};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "EventOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 14] = [
    "seq",
    "id",
    "workspace_id",
    "event_type",
    "entity_type",
    "entity_id",
    "actor",
    "actor_kind",
    "occurred_at",
    "correlation_id",
    "causation_id",
    "attempt_id",
    "schema_version",
    "data",
];

/// An event as stored. `seq` is the sequence number the store assigned on append; every other
/// column comes from the draft the decision produced.
pub fn from_model(seq: i64, draft: &EventDraft) -> Result<Value, WgError> {
    Ok(model(
        NAME,
        vec![
            ("seq", Value::Int(i128::from(seq))),
            ("id", id(draft.id)),
            ("workspace_id", optional_id(draft.workspace_id)),
            ("event_type", text(draft.event_type())),
            ("entity_type", text(draft.entity_type().as_str())),
            ("entity_id", id(draft.entity_id)),
            ("actor", text(&draft.actor.to_string())),
            ("actor_kind", text(draft.actor.kind().as_str())),
            (
                "occurred_at",
                timestamp(draft.occurred_at, TimeFormat::ModelZ),
            ),
            ("correlation_id", id(draft.correlation_id)),
            ("causation_id", optional_id(draft.causation_id)),
            ("attempt_id", optional_id(draft.attempt_id)),
            (
                "schema_version",
                Value::Int(i128::from(draft.schema_version())),
            ),
            ("data", json_object(&draft.data()?)),
        ],
    ))
}

/// An optional id field: `null` when absent.
fn optional_id(value: Option<Id>) -> Value {
    value.map_or(Value::Null, id)
}
