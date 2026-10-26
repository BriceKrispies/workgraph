//! The verification area's outputs: `VerificationConfigOut`, `CheckResultOut`, `VerificationRunOut`,
//! `AcceptanceOut` (the Pydantic models of `verification/service.py`) and the failure row of
//! `VerificationService.failures` (a plain dict).
//!
//! These routes return untyped values, so the OpenAPI snapshot has no component schema for them;
//! [`NAME`] is the Python class name and the DTOs are not listed in [`super::implemented`].
//! Pydantic datetimes print in the `Z` form (`TimeFormat::ModelZ`); the failure row calls
//! `isoformat()` itself and so uses `TimeFormat::Isoformat`.

use wg_json::Value;
use wg_model::entity::JsonObject;
use wg_model::entity::verification::{
    TaskAcceptance, VerificationCheckResult, VerificationConfig, VerificationRun,
};
use wg_types::Id;

use crate::convention::{TimeFormat, id, json_object, model, optional_timestamp, text, timestamp};

/// The Python class of the config DTO.
pub const CONFIG_NAME: &str = "VerificationConfigOut";
/// The fields of [`CONFIG_NAME`] in declaration order.
pub const CONFIG_FIELDS: [&str; 11] = [
    "id",
    "workspace_id",
    "scope_type",
    "scope_id",
    "name",
    "checks",
    "enabled",
    "approved_by",
    "version",
    "created_at",
    "updated_at",
];

/// The Python class of a check result inside a run.
pub const CHECK_NAME: &str = "CheckResultOut";
/// The fields of [`CHECK_NAME`] in declaration order.
pub const CHECK_FIELDS: [&str; 12] = [
    "id",
    "name",
    "kind",
    "required",
    "repository_id",
    "status",
    "exit_code",
    "duration_ms",
    "timed_out",
    "log_artifact_id",
    "output_excerpt",
    "failure_signatures",
];

/// The Python class of the run DTO.
pub const RUN_NAME: &str = "VerificationRunOut";
/// The fields of [`RUN_NAME`] in declaration order.
pub const RUN_FIELDS: [&str; 13] = [
    "id",
    "workspace_id",
    "task_id",
    "attempt_id",
    "manifest_id",
    "manifest",
    "status",
    "required_passed",
    "triggered_by",
    "started_at",
    "finished_at",
    "created_at",
    "checks",
];

/// The Python class of the acceptance DTO.
pub const ACCEPTANCE_NAME: &str = "AcceptanceOut";
/// The fields of [`ACCEPTANCE_NAME`] in declaration order.
pub const ACCEPTANCE_FIELDS: [&str; 10] = [
    "id",
    "task_id",
    "attempt_id",
    "verification_run_id",
    "decision",
    "mode",
    "reason",
    "actor",
    "manifest_id",
    "created_at",
];

/// The keys of one row of `VerificationService.failures`, in the order the service builds them.
pub const FAILURE_FIELDS: [&str; 14] = [
    "check_result_id",
    "run_id",
    "task_id",
    "task_key",
    "attempt_id",
    "check",
    "kind",
    "required",
    "status",
    "exit_code",
    "timed_out",
    "failure_signatures",
    "repository_id",
    "created_at",
];

/// `VerificationConfigOut`: a config with its checks as stored. `created_at` and `updated_at` are
/// Pydantic datetimes.
pub fn config_from_model(config: &VerificationConfig) -> Value {
    model(
        CONFIG_NAME,
        vec![
            ("id", id(config.id)),
            ("workspace_id", id(config.workspace_id)),
            ("scope_type", text(&config.scope_type)),
            ("scope_id", id(config.scope_id)),
            ("name", text(&config.name)),
            ("checks", checks_list(config.checks.iter())),
            ("enabled", Value::Bool(config.enabled)),
            ("approved_by", text(&config.approved_by)),
            ("version", int(config.version)),
            (
                "created_at",
                timestamp(config.created_at, TimeFormat::ModelZ),
            ),
            (
                "updated_at",
                timestamp(config.updated_at, TimeFormat::ModelZ),
            ),
        ],
    )
}

/// `CheckResultOut`: one check of a run, as the run DTO lists it.
pub fn check_from_model(check: &VerificationCheckResult) -> Value {
    model(
        CHECK_NAME,
        vec![
            ("id", id(check.id)),
            ("name", text(&check.name)),
            ("kind", text(&check.kind)),
            ("required", Value::Bool(check.required)),
            ("repository_id", optional_id(check.repository_id)),
            ("status", text(&check.status)),
            ("exit_code", optional_int(check.exit_code)),
            ("duration_ms", optional_int(check.duration_ms)),
            ("timed_out", Value::Bool(check.timed_out)),
            ("log_artifact_id", optional_id(check.log_artifact_id)),
            ("output_excerpt", text(&check.output_excerpt)),
            (
                "failure_signatures",
                Value::List(check.failure_signatures.iter().map(|s| text(s)).collect()),
            ),
        ],
    )
}

/// `VerificationRunOut`. `manifest` is the entry list of the run's revision manifest (empty when
/// the run has none) and `checks` the run's check results in the order the caller lists them
/// (the service orders them by `created_at`).
pub fn run_from_model<'a>(
    run: &VerificationRun,
    manifest: impl IntoIterator<Item = &'a JsonObject>,
    checks: impl IntoIterator<Item = &'a VerificationCheckResult>,
) -> Value {
    model(
        RUN_NAME,
        vec![
            ("id", id(run.id)),
            ("workspace_id", id(run.workspace_id)),
            ("task_id", id(run.task_id)),
            ("attempt_id", optional_id(run.attempt_id)),
            ("manifest_id", optional_id(run.manifest_id)),
            (
                "manifest",
                Value::List(manifest.into_iter().map(json_object).collect()),
            ),
            ("status", text(&run.status)),
            ("required_passed", Value::Bool(run.required_passed)),
            ("triggered_by", text(&run.triggered_by)),
            (
                "started_at",
                optional_timestamp(run.started_at, TimeFormat::ModelZ),
            ),
            (
                "finished_at",
                optional_timestamp(run.finished_at, TimeFormat::ModelZ),
            ),
            ("created_at", timestamp(run.created_at, TimeFormat::ModelZ)),
            (
                "checks",
                Value::List(checks.into_iter().map(check_from_model).collect()),
            ),
        ],
    )
}

/// `AcceptanceOut`: one accept or reject decision.
pub fn acceptance_from_model(acceptance: &TaskAcceptance) -> Value {
    model(
        ACCEPTANCE_NAME,
        vec![
            ("id", id(acceptance.id)),
            ("task_id", id(acceptance.task_id)),
            ("attempt_id", optional_id(acceptance.attempt_id)),
            (
                "verification_run_id",
                optional_id(acceptance.verification_run_id),
            ),
            ("decision", text(&acceptance.decision)),
            ("mode", text(&acceptance.mode)),
            ("reason", text(&acceptance.reason)),
            ("actor", text(&acceptance.actor)),
            ("manifest_id", optional_id(acceptance.manifest_id)),
            (
                "created_at",
                timestamp(acceptance.created_at, TimeFormat::ModelZ),
            ),
        ],
    )
}

/// One row of `VerificationService.failures`: a failed check with its run and task. The task key
/// comes from the caller; `created_at` is printed with `isoformat()`, as the service does.
pub fn failure_from_model(
    check: &VerificationCheckResult,
    run: &VerificationRun,
    task_key: &str,
) -> Value {
    dict(vec![
        ("check_result_id", id(check.id)),
        ("run_id", id(run.id)),
        ("task_id", id(run.task_id)),
        ("task_key", text(task_key)),
        ("attempt_id", optional_id(run.attempt_id)),
        ("check", text(&check.name)),
        ("kind", text(&check.kind)),
        ("required", Value::Bool(check.required)),
        ("status", text(&check.status)),
        ("exit_code", optional_int(check.exit_code)),
        ("timed_out", Value::Bool(check.timed_out)),
        (
            "failure_signatures",
            Value::List(check.failure_signatures.iter().map(|s| text(s)).collect()),
        ),
        ("repository_id", optional_id(check.repository_id)),
        (
            "created_at",
            timestamp(check.created_at, TimeFormat::Isoformat),
        ),
    ])
}

fn checks_list<'a>(checks: impl IntoIterator<Item = &'a JsonObject>) -> Value {
    Value::List(checks.into_iter().map(json_object).collect())
}

fn optional_id(value: Option<Id>) -> Value {
    value.map_or(Value::Null, id)
}

fn optional_int(value: Option<i64>) -> Value {
    value.map_or(Value::Null, int)
}

fn int(value: i64) -> Value {
    Value::Int(i128::from(value))
}

fn dict(fields: Vec<(&str, Value)>) -> Value {
    Value::Dict(
        fields
            .into_iter()
            .map(|(key, value)| (wg_json::PyStr::from(key), value))
            .collect(),
    )
}
