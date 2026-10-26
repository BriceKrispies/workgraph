//! The spooled hook record (contract F09): what `claude hook` writes when it cannot ingest at once, the
//! file name it gets, and what a failed replay does to it (`hook_cli.py` and `HookIngestor.replay_spool`).

use serde_json::{Map, Value, json};

use crate::hook::INVALID_EVENT;
use crate::ingest::HookEnv;
use crate::json::{dumps_ordered, py_str};
use crate::redact::redact;

/// The bound on a payload kept in a spooled record.
pub const SPOOL_PAYLOAD_CHARS: usize = 20_000;
/// The longest failure reason a spooled record keeps.
pub const REASON_CHARS: usize = 500;
/// Failed replays after which a record is set aside as `.failed`.
pub const MAX_REPLAYS: i64 = 5;

/// A record as `claude hook` builds it: when it was received, the launch context, and the payload.
pub fn hook_record(received_at: &str, env: &HookEnv, payload: Value) -> Value {
    json!({
        "received_at": received_at,
        "env": {
            "attempt_id": env.attempt_id,
            "task_id": env.task_id,
            "workspace": env.workspace,
        },
        "payload": payload,
    })
}

/// The payload of input that is not JSON: the `Invalid` event with the parse error and an excerpt.
pub fn invalid_payload(error: &str, input: &[u8]) -> Value {
    json!({
        "hook_event_name": INVALID_EVENT,
        "error": error,
        "excerpt": String::from_utf8_lossy(&input[..input.len().min(200)]),
    })
}

/// The payload of input over the size limit: the `Oversized` event with its size.
pub fn oversized_payload(size: usize) -> Value {
    json!({"hook_event_name": "Oversized", "size": size})
}

/// `f"{type}: {message}"[:500]`: the failure text a spooled record keeps.
pub fn failure_reason(type_name: &str, message: &str) -> String {
    format!("{type_name}: {message}")
        .chars()
        .take(REASON_CHARS)
        .collect()
}

/// The record as it is spooled: its payload redacted and bounded, and the reason ingestion failed.
pub fn spooled(record: &Value, reason: &str) -> Value {
    let entries = record.as_object().cloned().unwrap_or_default();
    let redacted = entries
        .get("payload")
        .map_or(Value::Null, |payload| redact(payload, SPOOL_PAYLOAD_CHARS));
    Value::Object(set(
        &set(&entries, "payload", redacted),
        "spool_reason",
        json!(reason),
    ))
}

/// The spool file name: the received time without `:` and `-`, a dash, and the random `suffix`.
pub fn spool_name(record: &Value, suffix: &str) -> String {
    let received = py_str(record.get("received_at").unwrap_or(&Value::Null));
    format!("{}-{suffix}.json", received.replace([':', '-'], ""))
}

/// The text of a spooled record file: `json.dumps(record)`.
pub fn spool_text(record: &Value) -> String {
    dumps_ordered(record)
}

/// What a failed replay does with a record.
#[derive(Clone, Debug, PartialEq)]
pub enum Replayed {
    /// Write the record back in place; it will be tried again.
    Retry(Value),
    /// The record failed its fifth replay: write it as `.failed` and remove the original.
    SetAside(Value),
}

/// The record after one more failed replay, with the count and the last error.
pub fn after_failed_replay(record: &Value, reason: &str) -> Replayed {
    let entries = record.as_object().cloned().unwrap_or_default();
    let attempts = entries
        .get("replay_attempts")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        + 1;
    let updated = Value::Object(set(
        &set(&entries, "replay_attempts", json!(attempts)),
        "last_replay_error",
        json!(reason.chars().take(REASON_CHARS).collect::<String>()),
    ));
    if attempts >= MAX_REPLAYS {
        Replayed::SetAside(updated)
    } else {
        Replayed::Retry(updated)
    }
}

/// `map[key] = value`: an existing key keeps its position, a new one goes last.
fn set(map: &Map<String, Value>, key: &str, value: Value) -> Map<String, Value> {
    if map.contains_key(key) {
        map.iter()
            .map(|(name, old)| {
                (
                    name.clone(),
                    if name == key {
                        value.clone()
                    } else {
                        old.clone()
                    },
                )
            })
            .collect()
    } else {
        map.iter()
            .map(|(name, old)| (name.clone(), old.clone()))
            .chain(std::iter::once((key.to_owned(), value)))
            .collect()
    }
}
