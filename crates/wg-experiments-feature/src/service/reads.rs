//! Reads for the experiment family (SH-14): experiments, their trials and the predictor fits.
//!
//! The statements are built by [`crate::spec::sql`] as values; this module only runs
//! them through the session and decodes the rows with the table codecs. Writes go through the
//! generic [`Session::apply_write`] and [`Session::apply_decision`] like every other family.

use crate::spec::sql::{self, TrialWithTaskKey};
use wg_model::entity::{Experiment, ExperimentTrial, PredictorFit};
use wg_types::{Id, WgError};

use wg_store::Session;
use wg_store::query::{many, one};

/// The experiment whose id is `experiment_id`, if there is one.
pub fn experiment_by_id(
    session: &mut Session<'_>,
    experiment_id: Id,
) -> Result<Option<Experiment>, WgError> {
    one(session, &sql::experiment_by_id(experiment_id))
}

/// The experiment of `workspace_id` with `slug`, if there is one.
pub fn experiment_by_slug(
    session: &mut Session<'_>,
    workspace_id: Id,
    slug: &str,
) -> Result<Option<Experiment>, WgError> {
    one(session, &sql::experiment_by_slug(workspace_id, slug))
}

/// The experiments of `workspace_id`, oldest first; only those in `status` when one is given.
pub fn experiments_in_workspace(
    session: &mut Session<'_>,
    workspace_id: Id,
    status: Option<&str>,
) -> Result<Vec<Experiment>, WgError> {
    many(
        session,
        &sql::experiments_in_workspace(workspace_id, status),
    )
}

/// The trial whose id is `trial_id`, if there is one.
pub fn trial_by_id(
    session: &mut Session<'_>,
    trial_id: Id,
) -> Result<Option<ExperimentTrial>, WgError> {
    one(session, &sql::trial_by_id(trial_id))
}

/// The trials of an experiment in the order they were added.
pub fn trials_of_experiment(
    session: &mut Session<'_>,
    experiment_id: Id,
) -> Result<Vec<ExperimentTrial>, WgError> {
    many(session, &sql::trials_of_experiment(experiment_id))
}

/// The trials of an experiment in the order they were added, each with its task's key.
pub fn trials_with_task_keys(
    session: &mut Session<'_>,
    experiment_id: Id,
) -> Result<Vec<TrialWithTaskKey>, WgError> {
    session
        .rows(&sql::trials_with_task_keys(experiment_id))?
        .iter()
        .map(|values| TrialWithTaskKey::from_raw(values))
        .collect()
}

/// The number of trials an experiment has.
pub fn trial_count(session: &mut Session<'_>, experiment_id: Id) -> Result<i64, WgError> {
    let rows = session.rows(&sql::trial_count(experiment_id))?;
    Ok(wg_storecodec::sql::int_scalar(&rows).unwrap_or(0))
}

/// The highest trial `seq` of an experiment, if it has any trials.
pub fn max_trial_seq(session: &mut Session<'_>, experiment_id: Id) -> Result<Option<i64>, WgError> {
    let rows = session.rows(&sql::max_trial_seq(experiment_id))?;
    Ok(wg_storecodec::sql::int_scalar(&rows))
}

/// The predictor fit whose id is `fit_id`, if there is one.
pub fn predictor_fit_by_id(
    session: &mut Session<'_>,
    fit_id: Id,
) -> Result<Option<PredictorFit>, WgError> {
    one(session, &sql::predictor_fit_by_id(fit_id))
}

/// The newest fit of `model` in `workspace_id`, if there is one.
pub fn latest_predictor_fit(
    session: &mut Session<'_>,
    workspace_id: Id,
    model: &str,
) -> Result<Option<PredictorFit>, WgError> {
    one(session, &sql::latest_predictor_fit(workspace_id, model))
}
