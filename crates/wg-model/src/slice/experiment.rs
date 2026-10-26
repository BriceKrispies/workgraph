//! [`ExperimentSlice`]: experiments, their trials and predictor fits.

use rpds::RedBlackTreeMapSync;
use wg_types::{Id, Timestamp};

use crate::entity::{Experiment, ExperimentTrial, PredictorFit};
use crate::write::{Row, RowKey, Write};

/// Loaded rows of the experiment family, keyed by primary key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExperimentSlice {
    pub experiments: RedBlackTreeMapSync<Id, Experiment>,
    pub trials: RedBlackTreeMapSync<Id, ExperimentTrial>,
    pub predictor_fits: RedBlackTreeMapSync<Id, PredictorFit>,
}

impl ExperimentSlice {
    pub fn experiment(&self, id: Id) -> Option<&Experiment> {
        self.experiments.get(&id)
    }

    /// The experiment of a workspace with this slug.
    pub fn experiment_by_slug(&self, workspace_id: Id, slug: &str) -> Option<&Experiment> {
        self.experiments
            .values()
            .find(|row| row.workspace_id == workspace_id && row.slug == slug)
    }

    /// The trials of an experiment, by trial id.
    pub fn trials_of(&self, experiment_id: Id) -> impl Iterator<Item = &ExperimentTrial> {
        self.trials
            .values()
            .filter(move |row| row.experiment_id == experiment_id)
    }

    /// The most recent fit of a workspace, by creation time.
    pub fn latest_fit(&self, workspace_id: Id) -> Option<&PredictorFit> {
        self.predictor_fits
            .values()
            .filter(|row| row.workspace_id == workspace_id)
            .max_by_key(|row| created_key(&row.created_at))
    }

    /// The slice after `write`. Writes to tables this slice does not hold leave it unchanged.
    #[must_use]
    pub fn apply(&self, write: &Write) -> Self {
        match write {
            Write::Insert(row) | Write::Update { row, .. } => match row {
                Row::Experiment(row) => ExperimentSlice {
                    experiments: self.experiments.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::ExperimentTrial(row) => ExperimentSlice {
                    trials: self.trials.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::PredictorFit(row) => ExperimentSlice {
                    predictor_fits: self.predictor_fits.insert(row.id, row.clone()),
                    ..self.clone()
                },
                _ => self.clone(),
            },
            Write::Delete(key) => match key {
                RowKey::Experiment(id) => ExperimentSlice {
                    experiments: self.experiments.remove(id),
                    ..self.clone()
                },
                RowKey::ExperimentTrial(id) => ExperimentSlice {
                    trials: self.trials.remove(id),
                    ..self.clone()
                },
                RowKey::PredictorFit(id) => ExperimentSlice {
                    predictor_fits: self.predictor_fits.remove(id),
                    ..self.clone()
                },
                _ => self.clone(),
            },
        }
    }
}

/// The sort key of a creation time: its UTC instant as an ISO string, which sorts in time order.
fn created_key(at: &Timestamp) -> String {
    at.format_isoformat()
}
