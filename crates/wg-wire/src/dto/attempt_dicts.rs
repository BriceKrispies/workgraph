//! The dicts the attempt routes return outside `AttemptOut`: `GET /attempts/{id}/manifests`,
//! `GET /attempts/compare` and `GET /attempts/{id}/log`.
//!
//! The routes return plain mappings (`AttemptService.manifests`, `AttemptService.compare` and
//! `attempt_log`), so they have no OpenAPI component. A manifest's `created_at` is Python's
//! `isoformat()` text (`+00:00`); nothing else here is a datetime.

use serde_json::Value as Json;
use wg_json::{PyStr, Value, dict};
use wg_model::entity::run::{AgentAttempt, RevisionManifest};

use crate::convention::{TimeFormat, id, int, json_object, json_value, text, timestamp};

/// The name of the manifest dict; it has no OpenAPI component schema.
pub const MANIFEST_NAME: &str = "ManifestOut";

/// The keys of one manifest, in the order `AttemptService.manifests` builds them.
pub const MANIFEST_FIELDS: [&str; 4] = ["id", "kind", "entries", "created_at"];

/// The keys of the comparison, in the order `AttemptService.compare` builds them.
pub const COMPARE_FIELDS: [&str; 7] = [
    "a",
    "b",
    "same_task",
    "fields",
    "config_differences",
    "input_snapshot_same",
    "manifests",
];

/// The attempt fields the comparison lists, in order.
pub const COMPARED_FIELDS: [&str; 11] = [
    "status",
    "provider",
    "adapter",
    "model",
    "input_task_version",
    "duration_ms",
    "queue_wait_ms",
    "exit_reason",
    "usage",
    "cost_usd",
    "modified_repository_ids",
];

/// The keys of `GET /attempts/{id}/log` when the log exists.
pub const LOG_FIELDS: [&str; 3] = ["lines", "available", "total_lines"];

/// One manifest of an attempt.
pub fn manifest(row: &RevisionManifest) -> Value {
    dict!({
        "id": id(row.id),
        "kind": text(&row.kind),
        "entries": Value::List(row.entries.iter().map(json_object).collect()),
        "created_at": timestamp(row.created_at, TimeFormat::Isoformat),
    })
}

/// The manifests of an attempt, in the order given (`created_at` ascending).
pub fn manifests(rows: &[RevisionManifest]) -> Value {
    Value::List(rows.iter().map(manifest).collect())
}

/// The value of one compared field, as JSON.
fn compared(attempt: &AgentAttempt, field: &str) -> Json {
    let optional_text =
        |value: &Option<String>| value.as_ref().map_or(Json::Null, |v| v.clone().into());
    let optional_int = |value: Option<i64>| value.map_or(Json::Null, Json::from);
    match field {
        "status" => attempt.status.as_str().into(),
        "provider" => attempt.provider.clone().into(),
        "adapter" => attempt.adapter.clone().into(),
        "model" => optional_text(&attempt.model),
        "input_task_version" => optional_int(attempt.input_task_version),
        "duration_ms" => optional_int(attempt.duration_ms),
        "queue_wait_ms" => optional_int(attempt.queue_wait_ms),
        "exit_reason" => optional_text(&attempt.exit_reason),
        "usage" => Json::Object(attempt.usage.clone()),
        "cost_usd" => attempt
            .cost_usd
            .and_then(|cost| serde_json::Number::from_f64(cost.0))
            .map_or(Json::Null, Json::Number),
        "modified_repository_ids" => Json::Array(
            attempt
                .modified_repository_ids
                .iter()
                .map(|v| Json::from(v.clone()))
                .collect(),
        ),
        _ => Json::Null,
    }
}

/// The manifests of one attempt keyed by kind; a later manifest of the same kind replaces the
/// earlier one in place, as a dict comprehension does.
fn by_kind(rows: &[RevisionManifest]) -> Value {
    let mut entries: Vec<(PyStr, Value)> = Vec::new();
    for row in rows {
        let value = Value::List(row.entries.iter().map(json_object).collect());
        let key = PyStr::from(row.kind.as_str());
        match entries.iter_mut().find(|(existing, _)| *existing == key) {
            Some(slot) => slot.1 = value,
            None => entries.push((key, value)),
        }
    }
    Value::Dict(entries)
}

/// The side-by-side comparison of two attempts of one workspace.
pub fn compare(
    a: &AgentAttempt,
    b: &AgentAttempt,
    manifests_a: &[RevisionManifest],
    manifests_b: &[RevisionManifest],
) -> Value {
    let fields = COMPARED_FIELDS
        .iter()
        .map(|name| {
            let (left, right) = (compared(a, name), compared(b, name));
            let same = left == right;
            (
                PyStr::from(*name),
                dict!({
                    "a": json_value(&left),
                    "b": json_value(&right),
                    "same": Value::Bool(same),
                }),
            )
        })
        .collect();

    // The union of the snapshot keys in sorted order, then only the keys whose values differ.
    let mut keys: Vec<&String> = a
        .config_snapshot
        .keys()
        .chain(b.config_snapshot.keys())
        .collect();
    keys.sort();
    keys.dedup();
    let config_differences = keys
        .into_iter()
        .filter(|key| a.config_snapshot.get(*key) != b.config_snapshot.get(*key))
        .map(|key| {
            let side = |attempt: &AgentAttempt| {
                attempt
                    .config_snapshot
                    .get(key)
                    .map_or(Value::Null, json_value)
            };
            (
                PyStr::from(key.as_str()),
                dict!({ "a": side(a), "b": side(b) }),
            )
        })
        .collect();

    dict!({
        "a": id(a.id),
        "b": id(b.id),
        "same_task": Value::Bool(a.task_id == b.task_id),
        "fields": Value::Dict(fields),
        "config_differences": Value::Dict(config_differences),
        "input_snapshot_same": Value::Bool(a.input_task_snapshot == b.input_task_snapshot),
        "manifests": dict!({
            "a": by_kind(manifests_a),
            "b": by_kind(manifests_b),
        }),
    })
}

/// `GET /attempts/{id}/log`. `lines` is the tail already cut; `total_lines` is the line count of
/// the whole log. A missing log answers `{"lines": [], "available": false}` (see [`log_missing`]).
pub fn log(lines: &[&str], total_lines: i64) -> Value {
    dict!({
        "lines": Value::List(lines.iter().map(|line| text(line)).collect()),
        "available": Value::Bool(true),
        "total_lines": int(total_lines),
    })
}

/// `GET /attempts/{id}/log` when the log file does not exist.
pub fn log_missing() -> Value {
    dict!({
        "lines": Value::List(Vec::new()),
        "available": Value::Bool(false),
    })
}
