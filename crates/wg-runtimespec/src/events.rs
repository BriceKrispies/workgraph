//! The event documents of the API (`api/routes/ops.py`, `core/services/history.py`): the query the
//! history route reads, the query one tick of the stream reads, and the rule for when the stream
//! sends a queue snapshot. The shell runs the queries; the parameters arrive validated.

use serde_json::{Map, Value};
use wg_storecodec::EventQuery;
use wg_types::{Id, parse_id};

/// The largest `limit` the history route reads (`limit=min(limit, 5000)`).
pub const HISTORY_LIMIT_CAP: i64 = 5000;

/// The rows one stream tick reads (`limit=200`).
pub const STREAM_BATCH: i64 = 200;

/// The stream sends a queue snapshot on every fifth tick, starting with the first (`ticks % 5`).
pub const SNAPSHOT_EVERY_TICKS: u64 = 5;

/// A string parameter, when present and not null.
pub fn text_param<'a>(params: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

/// The workspace reference of the path (`{ws}`): a slug or an id, as the route received it.
pub fn workspace_reference(params: &Map<String, Value>) -> String {
    text_param(params, "ws").unwrap_or_default().to_owned()
}

/// The `after_seq` parameter, when given.
pub fn after_seq(params: &Map<String, Value>) -> Option<i64> {
    params.get("after_seq").and_then(Value::as_i64)
}

/// The `max_events` parameter, when given: the stream ends after that many frames.
pub fn max_events(params: &Map<String, Value>) -> Option<u64> {
    params.get("max_events").and_then(Value::as_u64)
}

/// Whether the stream sends a queue snapshot on tick `tick` (counted from 0).
pub fn snapshot_due(tick: u64) -> bool {
    tick.is_multiple_of(SNAPSHOT_EVERY_TICKS)
}

/// The query of `GET /ws/{ws}/events` for the validated parameters, in workspace `workspace_id`.
///
/// `None` when `entity_id` is not an id: no event carries it, so the answer is empty. An empty
/// `entity_id` is no filter, as in Python (`if entity_id`).
pub fn history_query(params: &Map<String, Value>, workspace_id: Id) -> Option<EventQuery> {
    let entity_ids = match text_param(params, "entity_id").filter(|text| !text.is_empty()) {
        Some(text) => vec![parse_id(text).ok()?],
        None => Vec::new(),
    };
    Some(EventQuery {
        workspace_id: Some(workspace_id),
        entity_type: text_param(params, "entity_type").map(str::to_owned),
        entity_ids,
        event_type_prefix: text_param(params, "prefix").map(str::to_owned),
        after_seq: after_seq(params),
        limit: params
            .get("limit")
            .and_then(Value::as_i64)
            .unwrap_or(STREAM_BATCH)
            .min(HISTORY_LIMIT_CAP),
        descending: params
            .get("descending")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..EventQuery::default()
    })
}

/// The query of one stream tick: the events of `workspace_id` after `cursor`, oldest first.
pub fn stream_query(workspace_id: Id, cursor: i64) -> EventQuery {
    EventQuery {
        workspace_id: Some(workspace_id),
        after_seq: Some(cursor),
        limit: STREAM_BATCH,
        ..EventQuery::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ws() -> Id {
        parse_id("0190a1b2-0000-7000-8000-000000000001").unwrap_or_default()
    }

    fn object(value: &Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn history_query_reads_the_validated_parameters() {
        let params = object(
            &json!({"after_seq": 4, "entity_type": "task", "entity_id": null,
            "prefix": "task.", "limit": 9000, "descending": true}),
        );
        let query = history_query(&params, ws()).unwrap_or_default();
        assert_eq!(query.workspace_id, Some(ws()));
        assert_eq!(query.after_seq, Some(4));
        assert_eq!(query.entity_type.as_deref(), Some("task"));
        assert_eq!(query.event_type_prefix.as_deref(), Some("task."));
        assert_eq!(query.limit, HISTORY_LIMIT_CAP);
        assert!(query.descending);
        assert!(query.entity_ids.is_empty());
    }

    #[test]
    fn an_entity_id_that_is_not_an_id_matches_nothing() {
        let bad = object(&json!({"entity_id": "not-an-id", "limit": 200}));
        assert!(history_query(&bad, ws()).is_none());
        let empty = object(&json!({"entity_id": "", "limit": 200}));
        assert!(history_query(&empty, ws()).is_some());
    }

    #[test]
    fn snapshots_every_fifth_tick_from_the_first() {
        let due: Vec<u64> = (0..11).filter(|tick| snapshot_due(*tick)).collect();
        assert_eq!(due, [0, 5, 10]);
    }

    #[test]
    fn the_stream_reads_batches_after_its_cursor() {
        let query = stream_query(ws(), 7);
        assert_eq!(query.after_seq, Some(7));
        assert_eq!(query.limit, STREAM_BATCH);
        assert!(!query.descending);
    }
}
