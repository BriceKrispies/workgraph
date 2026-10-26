//! `TrialOut`: an experiment trial as `trial_start`, `trial_grade`, `trial_classify` and
//! `experiment_next` print it (the Pydantic model of `core/models/schemas.py`).
//!
//! The task key is not stored on the trial row; the caller joins it from the task, as Python's
//! `trial_out` does. No HTTP route serves trials, so the DTO is not listed in
//! [`super::implemented`]. Pydantic datetimes print in the `Z` form.

use wg_json::Value;
use wg_model::entity::ExperimentTrial;

use crate::convention::{
    TimeFormat, id, int, json_object, model, optional_id, optional_int, optional_text,
    optional_timestamp, text, timestamp,
};

/// The Python class of this DTO.
pub const NAME: &str = "TrialOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 24] = [
    "id",
    "experiment_id",
    "task_id",
    "task_key",
    "attempt_id",
    "subagent_run_id",
    "arm",
    "requested_model",
    "actual_model",
    "phase",
    "seq",
    "band",
    "features",
    "base_ref",
    "worktree_path",
    "status",
    "outcome",
    "outcome_detail",
    "failure_cause",
    "failure_cause_source",
    "notes",
    "created_at",
    "started_at",
    "graded_at",
];

/// A trial row as its DTO, with the task key the caller looked up.
pub fn from_model(trial: &ExperimentTrial, task_key: &str) -> Value {
    model(
        NAME,
        vec![
            ("id", id(trial.id)),
            ("experiment_id", id(trial.experiment_id)),
            ("task_id", id(trial.task_id)),
            ("task_key", text(task_key)),
            ("attempt_id", optional_id(trial.attempt_id)),
            ("subagent_run_id", optional_id(trial.subagent_run_id)),
            ("arm", text(&trial.arm)),
            ("requested_model", text(&trial.requested_model)),
            ("actual_model", optional_text(trial.actual_model.as_deref())),
            ("phase", text(&trial.phase)),
            ("seq", int(trial.seq)),
            ("band", optional_int(trial.band)),
            ("features", json_object(&trial.features)),
            ("base_ref", optional_text(trial.base_ref.as_deref())),
            (
                "worktree_path",
                optional_text(trial.worktree_path.as_deref()),
            ),
            ("status", text(&trial.status)),
            ("outcome", optional_text(trial.outcome.as_deref())),
            ("outcome_detail", json_object(&trial.outcome_detail)),
            (
                "failure_cause",
                optional_text(trial.failure_cause.as_deref()),
            ),
            (
                "failure_cause_source",
                optional_text(trial.failure_cause_source.as_deref()),
            ),
            ("notes", optional_text(trial.notes.as_deref())),
            (
                "created_at",
                timestamp(trial.created_at, TimeFormat::ModelZ),
            ),
            (
                "started_at",
                optional_timestamp(trial.started_at, TimeFormat::ModelZ),
            ),
            (
                "graded_at",
                optional_timestamp(trial.graded_at, TimeFormat::ModelZ),
            ),
        ],
    )
}
