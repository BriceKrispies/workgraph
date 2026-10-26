//! The job dicts of `GET /jobs` and `GET /jobs/{id}`.
//!
//! The routes return these as plain mappings (`_job` in `api/routes/ops.py`), not as Pydantic
//! models, but FastAPI encodes their datetimes like a model field, so they print in the `Z` form
//! (checked against a live Python response: `"run_at":"2026-10-26T04:55:43.038933Z"`).

use wg_json::Value;
use wg_model::entity::queue::{Job, JobRun};

use crate::convention::{
    TimeFormat, dict, id, int, json_object, optional_id, optional_text, optional_timestamp, text,
    texts, timestamp,
};

/// The name of the job dict; it has no OpenAPI component schema.
pub const NAME: &str = "JobOut";

/// The fields of a job, in the order `_job` lists them.
pub const FIELDS: [&str; 23] = [
    "id",
    "workspace_id",
    "kind",
    "status",
    "priority",
    "payload",
    "result",
    "run_at",
    "attempts",
    "max_attempts",
    "lease_owner",
    "lease_expires_at",
    "heartbeat_at",
    "started_at",
    "finished_at",
    "cancel_requested",
    "resource_keys",
    "last_error",
    "error_kind",
    "task_id",
    "attempt_id",
    "created_by",
    "created_at",
];

/// The fields of `GET /jobs/{id}`: the job fields, then `runs`.
pub const DETAIL_FIELDS: [&str; 24] = [
    "id",
    "workspace_id",
    "kind",
    "status",
    "priority",
    "payload",
    "result",
    "run_at",
    "attempts",
    "max_attempts",
    "lease_owner",
    "lease_expires_at",
    "heartbeat_at",
    "started_at",
    "finished_at",
    "cancel_requested",
    "resource_keys",
    "last_error",
    "error_kind",
    "task_id",
    "attempt_id",
    "created_by",
    "created_at",
    "runs",
];

/// The fields of one entry of a job's `runs` list, in the order the route builds them.
pub const RUN_FIELDS: [&str; 6] = [
    "attempt",
    "worker",
    "status",
    "error",
    "started_at",
    "finished_at",
];

/// A job as the API returns it.
pub fn from_model(job: &Job) -> Value {
    dict(job_fields(job))
}

/// A job with its runs, as `GET /jobs/{id}` returns it.
pub fn detail_from_model(job: &Job, runs: &[JobRun]) -> Value {
    let mut fields = job_fields(job);
    fields.push((
        "runs",
        Value::List(runs.iter().map(run_from_model).collect()),
    ));
    dict(fields)
}

/// One run of a job, as the detail route lists it.
pub fn run_from_model(run: &JobRun) -> Value {
    dict(vec![
        ("attempt", int(run.attempt_number)),
        ("worker", text(&run.worker_id)),
        ("status", text(&run.status)),
        ("error", optional_text(run.error.as_deref())),
        ("started_at", timestamp(run.started_at, TimeFormat::Z)),
        (
            "finished_at",
            optional_timestamp(run.finished_at, TimeFormat::Z),
        ),
    ])
}

fn job_fields(job: &Job) -> Vec<(&'static str, Value)> {
    vec![
        ("id", id(job.id)),
        ("workspace_id", optional_id(job.workspace_id)),
        ("kind", text(&job.kind)),
        ("status", text(job.status.as_str())),
        ("priority", int(job.priority)),
        ("payload", json_object(&job.payload)),
        (
            "result",
            job.result.as_ref().map_or(Value::Null, json_object),
        ),
        ("run_at", timestamp(job.run_at, TimeFormat::Z)),
        ("attempts", int(job.attempts)),
        ("max_attempts", int(job.max_attempts)),
        ("lease_owner", optional_text(job.lease_owner.as_deref())),
        (
            "lease_expires_at",
            optional_timestamp(job.lease_expires_at, TimeFormat::Z),
        ),
        (
            "heartbeat_at",
            optional_timestamp(job.heartbeat_at, TimeFormat::Z),
        ),
        (
            "started_at",
            optional_timestamp(job.started_at, TimeFormat::Z),
        ),
        (
            "finished_at",
            optional_timestamp(job.finished_at, TimeFormat::Z),
        ),
        ("cancel_requested", Value::Bool(job.cancel_requested)),
        (
            "resource_keys",
            texts(job.resource_keys.iter().map(String::as_str)),
        ),
        ("last_error", optional_text(job.last_error.as_deref())),
        ("error_kind", optional_text(job.error_kind.as_deref())),
        ("task_id", optional_id(job.task_id)),
        ("attempt_id", optional_id(job.attempt_id)),
        ("created_by", text(&job.created_by)),
        ("created_at", timestamp(job.created_at, TimeFormat::Z)),
    ]
}
