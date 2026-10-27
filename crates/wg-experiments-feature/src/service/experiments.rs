//! The experiment reads of the application (IF-19): `experiment list` and `experiment show`.
//! Free functions over `&mut App`; each checks the read permission, resolves the workspace, reads
//! through the store and returns the JSON the Python `ExperimentService` returned.

use crate::spec::status_choice;
use wg_json::Value;
use wg_model::entity::Experiment;
use wg_model::py_repr_str;
use wg_runtime::{App, RequestContext};
use wg_store::Session;
use wg_types::{Id, WgError, parse_id};
use wg_wire::dto::{experiment, experiment_shapes, trial};

use super::reads;

/// `ExperimentService.list`: the experiments of the workspace, oldest first, each as an
/// `ExperimentOut`. `status` must be one of [`STATUSES`] when given.
pub fn experiment_list(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    status: Option<&str>,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    let status = status.map(status_choice).transpose()?;
    app.runtime.query(
        |session| reads::experiments_in_workspace(session, scope, status.as_deref()),
        |rows| {
            Ok(Value::List(
                rows.iter().map(experiment::from_model).collect(),
            ))
        },
    )
}

/// `ExperimentService.get` with `trials`: `{"experiment": ..., "trials": [...]}`. `reference` is
/// an experiment id or a slug of the workspace.
pub fn experiment_show(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| {
            let found = find(session, scope, reference)?;
            let trials = reads::trials_with_task_keys(session, found.id)?;
            Ok((found, trials))
        },
        |(found, trials)| {
            let trial_rows = trials
                .iter()
                .map(|row| trial::from_model(&row.trial, &row.task_key))
                .collect();
            Ok(experiment_shapes::experiment(
                experiment::from_model(found),
                trial_rows,
            ))
        },
    )
}

/// `ExperimentService._find`: an id (when the reference parses as one) or a slug, in the
/// workspace. Anything else is a `not_found` error with the reference as its detail.
pub(crate) fn find(
    session: &mut Session<'_>,
    workspace_id: Id,
    reference: &str,
) -> Result<Experiment, WgError> {
    let found = match parse_id(reference) {
        Ok(experiment_id) => reads::experiment_by_id(session, experiment_id)?,
        Err(_) => reads::experiment_by_slug(session, workspace_id, reference)?,
    };
    match found {
        Some(experiment) if experiment.workspace_id == workspace_id => Ok(experiment),
        _ => Err(
            WgError::not_found(format!("experiment {} not found", py_repr_str(reference)))
                .detail("ref", serde_json::Value::String(reference.to_owned())),
        ),
    }
}
