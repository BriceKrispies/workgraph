//! SQL text and parameters for the experiment family (SH-14): experiments, their trials and the
//! predictor fits. The lookups the experiment services make by id, slug and workspace, the trial
//! list of an experiment (joined to the task key the trial output shows), the count and sequence
//! maximum that adding and closing a trial read, and the latest fit of a model.
//!
//! Every SELECT lists the table's [`columns`] in schema order, so its rows decode through the same
//! `TableRow` codecs as a fetch by key. Ordering follows the Python queries; where a key is unique
//! the Python query has no tie to break, and where it orders by a non-unique column the unique `id`
//! is appended as a tiebreak (noted on each function).

use wg_model::Table;
use wg_model::entity::ExperimentTrial;
use wg_types::{Id, WgError};

use crate::codec::{DbFailure, Raw, db_error, id_sql, text_sql};
use crate::rows::{TableRow, columns};
use crate::sql::Statement;

/// The table's columns, quoted and qualified by `alias` when one is given.
fn select_list(table: Table, alias: Option<&str>) -> String {
    columns(table)
        .iter()
        .map(|column| match alias {
            Some(alias) => format!("{alias}.\"{column}\""),
            None => format!("\"{column}\""),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The experiment whose id is `experiment_id`.
pub fn experiment_by_id(experiment_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"experiments\" WHERE \"id\" = ?1",
            select_list(Table::Experiment, None)
        ),
        params: vec![id_sql(experiment_id)],
    }
}

/// The experiment of `workspace_id` with `slug` (unique per workspace).
pub fn experiment_by_slug(workspace_id: Id, slug: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"experiments\" WHERE \"workspace_id\" = ?1 AND \"slug\" = ?2",
            select_list(Table::Experiment, None)
        ),
        params: vec![id_sql(workspace_id), text_sql(slug)],
    }
}

/// The experiments of `workspace_id`, optionally only those in `status`, oldest first. The Python
/// query orders by `created_at` alone; `id` is appended as a tiebreak.
pub fn experiments_in_workspace(workspace_id: Id, status: Option<&str>) -> Statement {
    let columns = select_list(Table::Experiment, None);
    match status {
        Some(status) => Statement {
            sql: format!(
                "SELECT {columns} FROM \"experiments\" WHERE \"workspace_id\" = ?1 \
                 AND \"status\" = ?2 ORDER BY \"created_at\", \"id\""
            ),
            params: vec![id_sql(workspace_id), text_sql(status)],
        },
        None => Statement {
            sql: format!(
                "SELECT {columns} FROM \"experiments\" WHERE \"workspace_id\" = ?1 \
                 ORDER BY \"created_at\", \"id\""
            ),
            params: vec![id_sql(workspace_id)],
        },
    }
}

/// The trial whose id is `trial_id`.
pub fn trial_by_id(trial_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"experiment_trials\" WHERE \"id\" = ?1",
            select_list(Table::ExperimentTrial, None)
        ),
        params: vec![id_sql(trial_id)],
    }
}

/// The trials of an experiment in the order they were added (`seq`, unique per experiment).
pub fn trials_of_experiment(experiment_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"experiment_trials\" WHERE \"experiment_id\" = ?1 ORDER BY \"seq\"",
            select_list(Table::ExperimentTrial, None)
        ),
        params: vec![id_sql(experiment_id)],
    }
}

/// The trials of an experiment in `seq` order, each joined to the key of its task. The trial's
/// columns come first, in [`columns`] order, and the task key is the last column.
pub fn trials_with_task_keys(experiment_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {}, \"tasks\".\"key\" FROM \"experiment_trials\" \
             JOIN \"tasks\" ON \"tasks\".\"id\" = \"experiment_trials\".\"task_id\" \
             WHERE \"experiment_trials\".\"experiment_id\" = ?1 \
             ORDER BY \"experiment_trials\".\"seq\"",
            select_list(Table::ExperimentTrial, Some("\"experiment_trials\""))
        ),
        params: vec![id_sql(experiment_id)],
    }
}

/// The number of trials an experiment has (one row, one integer column).
pub fn trial_count(experiment_id: Id) -> Statement {
    Statement {
        sql: "SELECT COUNT(*) FROM \"experiment_trials\" WHERE \"experiment_id\" = ?1".to_owned(),
        params: vec![id_sql(experiment_id)],
    }
}

/// The highest trial `seq` of an experiment (one row; NULL when it has no trials).
pub fn max_trial_seq(experiment_id: Id) -> Statement {
    Statement {
        sql: "SELECT MAX(\"seq\") FROM \"experiment_trials\" WHERE \"experiment_id\" = ?1"
            .to_owned(),
        params: vec![id_sql(experiment_id)],
    }
}

/// The predictor fit whose id is `fit_id`.
pub fn predictor_fit_by_id(fit_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"predictor_fits\" WHERE \"id\" = ?1",
            select_list(Table::PredictorFit, None)
        ),
        params: vec![id_sql(fit_id)],
    }
}

/// The newest fit of `model` in `workspace_id` (at most one row). Python orders by `created_at`
/// descending; `id` descending is the tiebreak.
pub fn latest_predictor_fit(workspace_id: Id, model: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"predictor_fits\" WHERE \"workspace_id\" = ?1 AND \"model\" = ?2 \
             ORDER BY \"created_at\" DESC, \"id\" DESC LIMIT 1",
            select_list(Table::PredictorFit, None)
        ),
        params: vec![id_sql(workspace_id), text_sql(model)],
    }
}

/// A trial with the key of the task it measures, from [`trials_with_task_keys`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialWithTaskKey {
    pub trial: ExperimentTrial,
    pub task_key: String,
}

impl TrialWithTaskKey {
    /// Decodes one joined row: the trial's columns, then the task key.
    pub fn from_raw(values: &[Raw]) -> Result<Self, WgError> {
        let width = columns(Table::ExperimentTrial).len();
        let trial = ExperimentTrial::from_raw(&values[..width.min(values.len())])?;
        match values.get(width) {
            Some(Raw::Text(task_key)) => Ok(TrialWithTaskKey {
                trial,
                task_key: task_key.clone(),
            }),
            _ => Err(db_error(
                DbFailure::Other,
                "a joined trial row lacks its task key",
            )),
        }
    }
}

/// The integer in the first column of the first row of a one-row result, if there is one and it
/// is not NULL.
pub fn int_scalar(rows: &[Vec<Raw>]) -> Option<i64> {
    match rows.first().and_then(|row| row.first()) {
        Some(Raw::Int(value)) => Some(*value),
        _ => None,
    }
}
