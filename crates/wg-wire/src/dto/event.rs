//! `EventOut`: one row of the `events` table, as the history endpoints and the SSE stream print it.
//!
//! The row comes from the store as a [`StoredEvent`] (its `data` keys sorted, as stored). Two
//! forms exist, as in Python:
//!
//! * [`from_stored`] is the `EventOut` model of the history endpoints (`occurred_at` in the `Z`
//!   form).
//! * [`dict_from_stored`] is `Event.to_dict()`, which `run_get` puts in `recent_events`: a plain
//!   dict whose `occurred_at` is `isoformat()` text (`+00:00`).

use wg_json::Value;
use wg_model::event::{EventDraft, StoredEvent};
use wg_types::WgError;

use crate::convention::{
    TimeFormat, dict, id, int, json_object, model, optional_id, text, timestamp,
};

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

/// An event as the store holds it right after appending `draft` with sequence number `seq`.
pub fn from_model(seq: i64, draft: &EventDraft) -> Result<Value, WgError> {
    Ok(from_stored(&StoredEvent::from_draft(seq, draft)?))
}

/// `EventOut`: the event as the history endpoints return it.
pub fn from_stored(event: &StoredEvent) -> Value {
    model(NAME, fields(event, TimeFormat::ModelZ))
}

/// `Event.to_dict()`: the same keys as a plain dict with `isoformat()` text for `occurred_at`.
pub fn dict_from_stored(event: &StoredEvent) -> Value {
    dict(fields(event, TimeFormat::Isoformat))
}

fn fields(event: &StoredEvent, format: TimeFormat) -> Vec<(&'static str, Value)> {
    vec![
        ("seq", int(event.seq)),
        ("id", id(event.id)),
        ("workspace_id", optional_id(event.workspace_id)),
        ("event_type", text(&event.event_type)),
        ("entity_type", text(&event.entity_type)),
        ("entity_id", id(event.entity_id)),
        ("actor", text(&event.actor)),
        ("actor_kind", text(&event.actor_kind)),
        ("occurred_at", timestamp(event.occurred_at, format)),
        ("correlation_id", id(event.correlation_id)),
        ("causation_id", optional_id(event.causation_id)),
        ("attempt_id", optional_id(event.attempt_id)),
        ("schema_version", int(event.schema_version)),
        ("data", json_object(&event.data)),
    ]
}
