//! `AttemptOut`, the page envelope of the attempt list.
//!
//! The OpenAPI snapshot has no component of this name (the routes return it untyped), so it is
//! not in `dto::implemented`.

use rpds::VectorSync;
use serde_json::{Map, Value as Json};
use wg_model::entity::run::AgentAttempt;
use wg_types::{AttemptStatus, Id, Timestamp};

use crate::convention::{json_map, model_z};
use crate::wire_dto;

wire_dto! {
    /// The attempt as the API returns it. `input_task_snapshot` and `prompt_artifact_id` stay out
    /// of the wire form, as they do in Python.
    pub struct AttemptOut<'a>("AttemptOut") value from_model(a: &'a AgentAttempt) {
        id: Id = a.id,
        workspace_id: Id = a.workspace_id,
        task_id: Id = a.task_id,
        number: i64 = a.number,
        status: AttemptStatus = a.status,
        provider: &'a str = &a.provider,
        adapter: &'a str = &a.adapter,
        agent_id: Option<&'a str> = a.agent_id.as_deref(),
        model: Option<&'a str> = a.model.as_deref(),
        session_id: Option<&'a str> = a.session_id.as_deref(),
        parent_attempt_id: Option<Id> = a.parent_attempt_id,
        retry_of_attempt_id: Option<Id> = a.retry_of_attempt_id,
        rerun_of_attempt_id: Option<Id> = a.rerun_of_attempt_id,
        created_by: &'a str = &a.created_by,
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = a.created_at,
        #[serde(serialize_with = "model_z")]
        queued_at: Option<Timestamp> = a.queued_at,
        #[serde(serialize_with = "model_z")]
        started_at: Option<Timestamp> = a.started_at,
        #[serde(serialize_with = "model_z")]
        finished_at: Option<Timestamp> = a.finished_at,
        queue_wait_ms: Option<i64> = a.queue_wait_ms,
        duration_ms: Option<i64> = a.duration_ms,
        exit_reason: Option<&'a str> = a.exit_reason.as_deref(),
        exit_code: Option<i64> = a.exit_code,
        error: Option<&'a str> = a.error.as_deref(),
        summary: Option<&'a str> = a.summary.as_deref(),
        input_task_version: Option<i64> = a.input_task_version,
        baseline_manifest_id: Option<Id> = a.baseline_manifest_id,
        outcome_manifest_id: Option<Id> = a.outcome_manifest_id,
        #[serde(serialize_with = "json_map")]
        usage: &'a Map<String, Json> = &a.usage,
        usage_source: Option<&'a str> = a.usage_source.as_deref(),
        cost_usd: Option<f64> = a.cost_usd.map(|cost| cost.0),
        cost_source: Option<&'a str> = a.cost_source.as_deref(),
        #[serde(serialize_with = "json_map")]
        telemetry: &'a Map<String, Json> = &a.telemetry,
        modified_repository_ids: &'a VectorSync<String> = &a.modified_repository_ids,
        #[serde(serialize_with = "json_map")]
        config_snapshot: &'a Map<String, Json> = &a.config_snapshot,
        #[serde(serialize_with = "json_map")]
        environment: &'a Map<String, Json> = &a.environment,
    }
}
