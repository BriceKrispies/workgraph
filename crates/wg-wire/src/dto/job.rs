//! The job dicts of `GET /jobs` and `GET /jobs/{id}`.
//!
//! The routes return these as plain mappings (`_job` in `api/routes/ops.py`), not as Pydantic
//! models, but FastAPI encodes their datetimes like a model field, so they print in the `Z` form
//! (checked against a live Python response: `"run_at":"2026-10-26T04:55:43.038933Z"`).

use rpds::VectorSync;
use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_model::entity::queue::{Job, JobRun};
use wg_types::{Id, JobStatus, Timestamp};

use crate::convention::{json_map, json_map_opt, z_text};
use crate::dto::Dto;
use crate::wire_dto;

wire_dto! {
    /// A job as the API returns it, in the order `_job` lists the fields. A dict with no OpenAPI
    /// component schema.
    pub struct JobOut<'a>("JobOut") dict from_model(j: &'a Job) {
        id: Id = j.id,
        workspace_id: Option<Id> = j.workspace_id,
        kind: &'a str = &j.kind,
        status: JobStatus = j.status,
        priority: i64 = j.priority,
        #[serde(serialize_with = "json_map")]
        payload: &'a Map<String, Json> = &j.payload,
        #[serde(serialize_with = "json_map_opt")]
        result: Option<&'a Map<String, Json>> = j.result.as_ref(),
        #[serde(serialize_with = "z_text")]
        run_at: Timestamp = j.run_at,
        attempts: i64 = j.attempts,
        max_attempts: i64 = j.max_attempts,
        lease_owner: Option<&'a str> = j.lease_owner.as_deref(),
        #[serde(serialize_with = "z_text")]
        lease_expires_at: Option<Timestamp> = j.lease_expires_at,
        #[serde(serialize_with = "z_text")]
        heartbeat_at: Option<Timestamp> = j.heartbeat_at,
        #[serde(serialize_with = "z_text")]
        started_at: Option<Timestamp> = j.started_at,
        #[serde(serialize_with = "z_text")]
        finished_at: Option<Timestamp> = j.finished_at,
        cancel_requested: bool = j.cancel_requested,
        resource_keys: &'a VectorSync<String> = &j.resource_keys,
        last_error: Option<&'a str> = j.last_error.as_deref(),
        error_kind: Option<&'a str> = j.error_kind.as_deref(),
        task_id: Option<Id> = j.task_id,
        attempt_id: Option<Id> = j.attempt_id,
        created_by: &'a str = &j.created_by,
        #[serde(serialize_with = "z_text")]
        created_at: Timestamp = j.created_at,
    }
}

wire_dto! {
    /// One run of a job, as the detail route lists it.
    pub struct RunOut<'a>("JobRunOut") dict run_from_model(r: &'a JobRun) {
        attempt: i64 = r.attempt_number,
        worker: &'a str = &r.worker_id,
        status: &'a str = &r.status,
        error: Option<&'a str> = r.error.as_deref(),
        #[serde(serialize_with = "z_text")]
        started_at: Timestamp = r.started_at,
        #[serde(serialize_with = "z_text")]
        finished_at: Option<Timestamp> = r.finished_at,
    }
}

wire_dto! {
    /// The fields of `GET /jobs/{id}`: the job fields, then `runs`.
    pub struct JobDetail<'a>("JobDetail") extends JobOut<'a>, fields_of JobOut<'static> {
        pub runs: Vec<RunOut<'a>>,
    }
}

/// A job with its runs, as `GET /jobs/{id}` returns it.
pub fn detail_from_model(job: &Job, runs: &[JobRun]) -> Value {
    JobDetail {
        base: JobOut::new(job),
        runs: runs.iter().map(RunOut::new).collect(),
    }
    .dict()
}
