//! The verification area's outputs: `VerificationConfigOut`, `CheckResultOut`, `VerificationRunOut`,
//! `AcceptanceOut` (the Pydantic models of `verification/service.py`) and the failure row of
//! `VerificationService.failures` (a plain dict).
//!
//! These routes return untyped values, so the OpenAPI snapshot has no component schema for them;
//! the DTOs are not listed in [`super::implemented`]. Pydantic datetimes print in the `Z` form
//! (`model_z`); the failure row calls `isoformat()` itself and so uses `isoformat`.

use rpds::VectorSync;
use serde::{Serialize, Serializer};
use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_model::entity::verification::{
    TaskAcceptance, VerificationCheckResult, VerificationConfig, VerificationRun,
};
use wg_types::{Id, Timestamp};

use crate::convention::{isoformat, json_object, model_z};
use crate::wire_dto;

type Object = Map<String, Json>;

/// A list of stored JSON objects.
fn objects<S: Serializer>(maps: &Vec<&Object>, serializer: S) -> Result<S::Ok, S::Error> {
    Value::List(maps.iter().map(|map| json_object(map)).collect()).serialize(serializer)
}

wire_dto! {
    /// `VerificationConfigOut`: a config with its checks as stored. `created_at` and `updated_at`
    /// are Pydantic datetimes.
    pub struct VerificationConfigOut<'a>("VerificationConfigOut")
    value config_from_model(c: &'a VerificationConfig) {
        id: Id = c.id,
        workspace_id: Id = c.workspace_id,
        scope_type: &'a str = &c.scope_type,
        scope_id: Id = c.scope_id,
        name: &'a str = &c.name,
        #[serde(serialize_with = "objects")]
        checks: Vec<&'a Object> = c.checks.iter().collect(),
        enabled: bool = c.enabled,
        approved_by: &'a str = &c.approved_by,
        version: i64 = c.version,
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = c.created_at,
        #[serde(serialize_with = "model_z")]
        updated_at: Timestamp = c.updated_at,
    }
}

wire_dto! {
    /// `CheckResultOut`: one check of a run, as the run DTO lists it.
    pub struct CheckResultOut<'a>("CheckResultOut") value check_from_model(c: &'a VerificationCheckResult) {
        id: Id = c.id,
        name: &'a str = &c.name,
        kind: &'a str = &c.kind,
        required: bool = c.required,
        repository_id: Option<Id> = c.repository_id,
        status: &'a str = &c.status,
        exit_code: Option<i64> = c.exit_code,
        duration_ms: Option<i64> = c.duration_ms,
        timed_out: bool = c.timed_out,
        log_artifact_id: Option<Id> = c.log_artifact_id,
        output_excerpt: &'a str = &c.output_excerpt,
        failure_signatures: &'a VectorSync<String> = &c.failure_signatures,
    }
}

wire_dto! {
    /// `VerificationRunOut`. `manifest` is the entry list of the run's revision manifest (empty
    /// when the run has none) and `checks` the run's check results in the order the caller lists
    /// them (the service orders them by `created_at`).
    pub struct VerificationRunOut<'a>("VerificationRunOut")
    value run_from_model(
        r: &'a VerificationRun,
        manifest: impl IntoIterator<Item = &'a Object>,
        checks: impl IntoIterator<Item = &'a VerificationCheckResult>
    ) {
        id: Id = r.id,
        workspace_id: Id = r.workspace_id,
        task_id: Id = r.task_id,
        attempt_id: Option<Id> = r.attempt_id,
        manifest_id: Option<Id> = r.manifest_id,
        #[serde(serialize_with = "objects")]
        manifest: Vec<&'a Object> = manifest.into_iter().collect(),
        status: &'a str = &r.status,
        required_passed: bool = r.required_passed,
        triggered_by: &'a str = &r.triggered_by,
        #[serde(serialize_with = "model_z")]
        started_at: Option<Timestamp> = r.started_at,
        #[serde(serialize_with = "model_z")]
        finished_at: Option<Timestamp> = r.finished_at,
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = r.created_at,
        checks: Vec<CheckResultOut<'a>> = checks.into_iter().map(CheckResultOut::new).collect(),
    }
}

wire_dto! {
    /// `AcceptanceOut`: one accept or reject decision.
    pub struct AcceptanceOut<'a>("AcceptanceOut") value acceptance_from_model(a: &'a TaskAcceptance) {
        id: Id = a.id,
        task_id: Id = a.task_id,
        attempt_id: Option<Id> = a.attempt_id,
        verification_run_id: Option<Id> = a.verification_run_id,
        decision: &'a str = &a.decision,
        mode: &'a str = &a.mode,
        reason: &'a str = &a.reason,
        actor: &'a str = &a.actor,
        manifest_id: Option<Id> = a.manifest_id,
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = a.created_at,
    }
}

wire_dto! {
    /// One row of `VerificationService.failures`: a failed check with its run and task, from the
    /// parts the joined failure query returns. The task key comes from the caller; `created_at`
    /// is printed with `isoformat()`, as the service does.
    pub struct FailureRow<'a>("FailureRow")
    dict failure_row(c: &'a VerificationCheckResult, run_id: Id, task_id: Id, attempt_id: Option<Id>, task_key: &'a str) {
        check_result_id: Id = c.id,
        run_id: Id = run_id,
        task_id: Id = task_id,
        task_key: &'a str = task_key,
        attempt_id: Option<Id> = attempt_id,
        check: &'a str = &c.name,
        kind: &'a str = &c.kind,
        required: bool = c.required,
        status: &'a str = &c.status,
        exit_code: Option<i64> = c.exit_code,
        timed_out: bool = c.timed_out,
        failure_signatures: &'a VectorSync<String> = &c.failure_signatures,
        repository_id: Option<Id> = c.repository_id,
        #[serde(serialize_with = "isoformat")]
        created_at: Timestamp = c.created_at,
    }
}

/// [`failure_row`] for a check and the run it belongs to.
pub fn failure_from_model(
    check: &VerificationCheckResult,
    run: &VerificationRun,
    task_key: &str,
) -> Value {
    failure_row(check, run.id, run.task_id, run.attempt_id, task_key)
}
