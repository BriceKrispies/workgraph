//! `AttemptOut`, the page envelope of the attempt list.

use wg_json::Value;
use wg_model::entity::run::AgentAttempt;

use crate::convention::{
    TimeFormat, id, int, json_object, model, optional_float, optional_id, optional_int,
    optional_text, optional_timestamp, text, texts, timestamp,
};

/// The OpenAPI component schema this DTO mirrors. The snapshot has no component of this name
/// (the routes return it untyped), so it is not in `dto::implemented`.
pub const NAME: &str = "AttemptOut";

/// The fields in Python declaration order (`src/workgraph/core/models/schemas.py`).
pub const FIELDS: [&str; 35] = [
    "id",
    "workspace_id",
    "task_id",
    "number",
    "status",
    "provider",
    "adapter",
    "agent_id",
    "model",
    "session_id",
    "parent_attempt_id",
    "retry_of_attempt_id",
    "rerun_of_attempt_id",
    "created_by",
    "created_at",
    "queued_at",
    "started_at",
    "finished_at",
    "queue_wait_ms",
    "duration_ms",
    "exit_reason",
    "exit_code",
    "error",
    "summary",
    "input_task_version",
    "baseline_manifest_id",
    "outcome_manifest_id",
    "usage",
    "usage_source",
    "cost_usd",
    "cost_source",
    "telemetry",
    "modified_repository_ids",
    "config_snapshot",
    "environment",
];

/// The attempt as the API returns it. `input_task_snapshot` and `prompt_artifact_id` stay out of
/// the wire form, as they do in Python.
pub fn from_model(attempt: &AgentAttempt) -> Value {
    model(
        NAME,
        vec![
            ("id", id(attempt.id)),
            ("workspace_id", id(attempt.workspace_id)),
            ("task_id", id(attempt.task_id)),
            ("number", int(attempt.number)),
            ("status", text(attempt.status.as_str())),
            ("provider", text(&attempt.provider)),
            ("adapter", text(&attempt.adapter)),
            ("agent_id", optional_text(attempt.agent_id.as_deref())),
            ("model", optional_text(attempt.model.as_deref())),
            ("session_id", optional_text(attempt.session_id.as_deref())),
            ("parent_attempt_id", optional_id(attempt.parent_attempt_id)),
            (
                "retry_of_attempt_id",
                optional_id(attempt.retry_of_attempt_id),
            ),
            (
                "rerun_of_attempt_id",
                optional_id(attempt.rerun_of_attempt_id),
            ),
            ("created_by", text(&attempt.created_by)),
            (
                "created_at",
                timestamp(attempt.created_at, TimeFormat::ModelZ),
            ),
            (
                "queued_at",
                optional_timestamp(attempt.queued_at, TimeFormat::ModelZ),
            ),
            (
                "started_at",
                optional_timestamp(attempt.started_at, TimeFormat::ModelZ),
            ),
            (
                "finished_at",
                optional_timestamp(attempt.finished_at, TimeFormat::ModelZ),
            ),
            ("queue_wait_ms", optional_int(attempt.queue_wait_ms)),
            ("duration_ms", optional_int(attempt.duration_ms)),
            ("exit_reason", optional_text(attempt.exit_reason.as_deref())),
            ("exit_code", optional_int(attempt.exit_code)),
            ("error", optional_text(attempt.error.as_deref())),
            ("summary", optional_text(attempt.summary.as_deref())),
            (
                "input_task_version",
                optional_int(attempt.input_task_version),
            ),
            (
                "baseline_manifest_id",
                optional_id(attempt.baseline_manifest_id),
            ),
            (
                "outcome_manifest_id",
                optional_id(attempt.outcome_manifest_id),
            ),
            ("usage", json_object(&attempt.usage)),
            (
                "usage_source",
                optional_text(attempt.usage_source.as_deref()),
            ),
            (
                "cost_usd",
                optional_float(attempt.cost_usd.map(|cost| cost.0)),
            ),
            ("cost_source", optional_text(attempt.cost_source.as_deref())),
            ("telemetry", json_object(&attempt.telemetry)),
            (
                "modified_repository_ids",
                texts(attempt.modified_repository_ids.iter().map(String::as_str)),
            ),
            ("config_snapshot", json_object(&attempt.config_snapshot)),
            ("environment", json_object(&attempt.environment)),
        ],
    )
}
