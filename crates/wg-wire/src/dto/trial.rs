//! `TrialOut`: an experiment trial as `trial_start`, `trial_grade`, `trial_classify` and
//! `experiment_next` print it (the Pydantic model of `core/models/schemas.py`).
//!
//! The task key is not stored on the trial row; the caller joins it from the task, as Python's
//! `trial_out` does. No HTTP route serves trials, so the DTO is not listed in
//! [`super::implemented`]. Pydantic datetimes print in the `Z` form.

use serde_json::{Map, Value as Json};
use wg_model::entity::ExperimentTrial;
use wg_types::{Id, Timestamp};

use crate::convention::{json_map, model_z};
use crate::wire_dto;

wire_dto! {
    /// A trial row as its DTO, with the task key the caller looked up.
    pub struct TrialOut<'a>("TrialOut") value from_model(t: &'a ExperimentTrial, task_key: &'a str) {
        id: Id = t.id,
        experiment_id: Id = t.experiment_id,
        task_id: Id = t.task_id,
        task_key: &'a str = task_key,
        attempt_id: Option<Id> = t.attempt_id,
        subagent_run_id: Option<Id> = t.subagent_run_id,
        arm: &'a str = &t.arm,
        requested_model: &'a str = &t.requested_model,
        actual_model: Option<&'a str> = t.actual_model.as_deref(),
        phase: &'a str = &t.phase,
        seq: i64 = t.seq,
        band: Option<i64> = t.band,
        #[serde(serialize_with = "json_map")]
        features: &'a Map<String, Json> = &t.features,
        base_ref: Option<&'a str> = t.base_ref.as_deref(),
        worktree_path: Option<&'a str> = t.worktree_path.as_deref(),
        status: &'a str = &t.status,
        outcome: Option<&'a str> = t.outcome.as_deref(),
        #[serde(serialize_with = "json_map")]
        outcome_detail: &'a Map<String, Json> = &t.outcome_detail,
        failure_cause: Option<&'a str> = t.failure_cause.as_deref(),
        failure_cause_source: Option<&'a str> = t.failure_cause_source.as_deref(),
        notes: Option<&'a str> = t.notes.as_deref(),
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = t.created_at,
        #[serde(serialize_with = "model_z")]
        started_at: Option<Timestamp> = t.started_at,
        #[serde(serialize_with = "model_z")]
        graded_at: Option<Timestamp> = t.graded_at,
    }
}
