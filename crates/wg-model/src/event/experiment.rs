//! Payloads of the `experiment.*`, `experiment_trial.*` and `predictor.*` events
//! (`core/services/experiments.py`, `experiments/predictor.py`).

use serde::Serialize;
use serde_json::Value;
use wg_types::Id;

use crate::entity::JsonObject;

/// `experiment.created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentCreated {
    pub arms: Value,
    pub slug: String,
}

/// `experiment.frozen`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentFrozen {
    pub protocol: JsonObject,
    pub slug: String,
}

/// `experiment.protocol_updated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentProtocolUpdated {
    pub protocol: JsonObject,
    pub slug: String,
}

/// `experiment.closed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentClosed {
    pub slug: String,
    pub trials: Value,
}

/// `experiment_trial.added`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentTrialAdded {
    pub arm: String,
    pub band: Option<i64>,
    pub experiment: String,
    pub phase: String,
    pub requested_model: String,
    pub seq: i64,
    pub task_key: String,
}

/// `experiment_trial.started`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentTrialStarted {
    pub base_ref: Option<String>,
    pub experiment: String,
    pub worktree_path: Option<String>,
}

/// `experiment_trial.linked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentTrialLinked {
    pub actual_model: Option<String>,
    pub attempt_id: Option<Id>,
    pub experiment: String,
    pub requested_model: String,
    pub subagent_run_id: Option<Id>,
}

/// `experiment_trial.graded`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentTrialGraded {
    pub experiment: String,
    pub outcome: Option<String>,
    pub status: String,
}

/// `experiment_trial.classified`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentTrialClassified {
    pub experiment: String,
    pub failure_cause: Option<String>,
    pub failure_cause_source: Option<String>,
}

/// `experiment_trial.invalidated`. The model keys are present only when the trial carried them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExperimentTrialInvalidated {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_model: Option<String>,
    pub experiment: String,
    pub failure_cause: Option<String>,
    pub failure_cause_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
}

/// `predictor.fitted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PredictorFitted {
    pub converged: bool,
    pub data_cutoff: Option<String>,
    pub model: String,
    pub n: i64,
    pub repositories: Value,
    pub samples: Value,
}
