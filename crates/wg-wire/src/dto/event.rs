//! `EventOut`: one row of the `events` table, as the history endpoints and the SSE stream print it.
//!
//! The row comes from the store as a [`StoredEvent`] (its `data` keys sorted, as stored). Two
//! forms exist, as in Python:
//!
//! * [`from_stored`] is the `EventOut` model of the history endpoints (`occurred_at` in the `Z`
//!   form).
//! * [`dict_from_stored`] is `Event.to_dict()`, which `run_get` puts in `recent_events`: a plain
//!   dict whose `occurred_at` is `isoformat()` text (`+00:00`).

use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_model::event::{EventDraft, StoredEvent};
use wg_types::{Id, WgError};

use crate::convention::{TimeFormat, json_map, timestamp};
use crate::dto::Dto;
use crate::wire_dto;

wire_dto! {
    /// A stored event. `occurred_at` is built in the form the shape needs: `Z` under the model,
    /// `isoformat()` in the dict.
    pub struct EventOut<'a>("EventOut") value in_format(e: &'a StoredEvent, format: TimeFormat) {
        seq: i64 = e.seq,
        id: Id = e.id,
        workspace_id: Option<Id> = e.workspace_id,
        event_type: &'a str = &e.event_type,
        entity_type: &'a str = &e.entity_type,
        entity_id: Id = e.entity_id,
        actor: &'a str = &e.actor,
        actor_kind: &'a str = &e.actor_kind,
        occurred_at: Value = timestamp(e.occurred_at, format),
        correlation_id: &'a str = e.correlation_id.as_str(),
        causation_id: Option<Id> = e.causation_id,
        attempt_id: Option<Id> = e.attempt_id,
        schema_version: i64 = e.schema_version,
        #[serde(serialize_with = "json_map")]
        data: &'a Map<String, Json> = &e.data,
    }
}

/// An event as the store holds it right after appending `draft` with sequence number `seq`.
pub fn from_model(seq: i64, draft: &EventDraft) -> Result<Value, WgError> {
    Ok(from_stored(&StoredEvent::from_draft(seq, draft)?))
}

/// `EventOut`: the event as the history endpoints return it.
pub fn from_stored(event: &StoredEvent) -> Value {
    in_format(event, TimeFormat::ModelZ)
}

/// `Event.to_dict()`: the same keys as a plain dict with `isoformat()` text for `occurred_at`.
pub fn dict_from_stored(event: &StoredEvent) -> Value {
    EventOut::new(event, TimeFormat::Isoformat).dict()
}
