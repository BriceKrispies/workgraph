//! The experiment family: `experiments`, `experiment_trials` and `predictor_fits`.

use rpds::VectorSync;
use serde_json::Value;
use wg_types::{Id, Timestamp};

use crate::entity::JsonObject;
use crate::write::{Row, RowKey};

/// A row of `experiments`: a named comparison with a protocol that is frozen before trials run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Experiment {
    pub id: Id,
    pub workspace_id: Id,
    pub slug: String,
    pub title: String,
    pub question: Option<String>,
    pub status: String,
    pub protocol: JsonObject,
    pub frozen_at: Option<Timestamp>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A row of `experiment_trials`: one planned or run trial of an experiment. Unique per experiment
/// and sequence number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentTrial {
    pub id: Id,
    pub experiment_id: Id,
    pub task_id: Id,
    pub attempt_id: Option<Id>,
    pub subagent_run_id: Option<Id>,
    pub arm: String,
    pub requested_model: String,
    pub actual_model: Option<String>,
    pub phase: String,
    pub seq: i64,
    pub band: Option<i64>,
    pub features: JsonObject,
    pub base_ref: Option<String>,
    pub worktree_path: Option<String>,
    pub status: String,
    /// The grade Python writes once the trial is graded.
    pub outcome: Option<String>,
    pub outcome_detail: JsonObject,
    pub failure_cause: Option<String>,
    pub failure_cause_source: Option<String>,
    pub notes: Option<String>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub graded_at: Option<Timestamp>,
}

/// A row of `predictor_fits`: one fit of the predictor. The schema does not fix the element shape of
/// the JSON list columns (`coef`, `penalty`, `samples`), so their elements are kept as JSON values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredictorFit {
    pub id: Id,
    pub workspace_id: Id,
    pub model: String,
    pub created_at: Timestamp,
    pub data_cutoff: Option<Timestamp>,
    /// JSON list of trial ids.
    pub trial_ids: VectorSync<String>,
    pub feature_names: VectorSync<String>,
    pub repositories: VectorSync<String>,
    pub coef: VectorSync<Value>,
    pub penalty: VectorSync<Value>,
    pub n: i64,
    pub converged: bool,
    pub metrics: JsonObject,
    pub samples: VectorSync<Value>,
}

impl Experiment {
    pub fn row_key(&self) -> RowKey {
        RowKey::Experiment(self.id)
    }
}

impl ExperimentTrial {
    pub fn row_key(&self) -> RowKey {
        RowKey::ExperimentTrial(self.id)
    }
}

impl PredictorFit {
    pub fn row_key(&self) -> RowKey {
        RowKey::PredictorFit(self.id)
    }
}

impl From<Experiment> for Row {
    fn from(row: Experiment) -> Self {
        Row::Experiment(row)
    }
}

impl From<ExperimentTrial> for Row {
    fn from(row: ExperimentTrial) -> Self {
        Row::ExperimentTrial(row)
    }
}

impl From<PredictorFit> for Row {
    fn from(row: PredictorFit) -> Self {
        Row::PredictorFit(row)
    }
}
