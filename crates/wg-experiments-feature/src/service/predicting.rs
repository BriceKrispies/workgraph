//! The predictor entry points of the application (IF-19): `experiment fit` stores a fit over the
//! graded trials of one model, and `experiment predict` reads a stored fit.

use wg_json::Value;
use wg_runtime::{App, RequestContext};
use wg_types::{Permission, WgError, parse_id};
use wg_wire::dto::predictor;

use super::reads;
use crate::spec::predict::{
    self, FitSettings, PredictRequest, check_settings, check_sizes, first_repositories,
};

/// `Predictor.fit`: fits the predictor for `model` over all graded trials, bootstraps it and
/// stores the fit.
pub fn experiment_fit(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    model: &str,
    settings: &FitSettings,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::ExperimentWrite)?;
    check_settings(
        settings.repo_penalty,
        settings.feature_penalty,
        settings.samples,
    )?;
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    let row = app
        .runtime
        .decide(
            context,
            |session| {
                let experiments = reads::experiments_in_workspace(session, scope, None)?;
                let trials = experiments
                    .iter()
                    .map(|experiment| reads::trials_of_experiment(session, experiment.id))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>();
                let world = session.task_world(Some(scope), &[])?;
                let firsts = first_repositories(&world.tasks);
                Ok(predict::fit_rows(model, &trials, |task_id| {
                    firsts.get(&task_id).copied()
                }))
            },
            |rows, stamp| predict::fit(stamp, scope, model, rows, settings),
        )?
        .reply;
    Ok(predictor::fit_from_model(&row))
}

/// `Predictor.predict_task`: the prediction for a task whose `contract.predicted` metadata gives
/// the features.
fn predict_task(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    model: &str,
    task_ref: &str,
    fit_id: Option<&str>,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let found = app.workspace_or_all(workspace.filter(|name| !name.is_empty()))?;
    let (key, metadata, repository) = app.runtime.query(
        |session| session.task_world(found, &[]),
        |world| {
            let task = wg_registry::resolve::task(&world.tasks, found, task_ref)?;
            let repository = first_repositories(&world.tasks).get(&task.id).copied();
            Ok((task.key.to_string(), task.metadata.clone(), repository))
        },
    )?;
    let (src_lines, src_files, packages) = predict::task_features(&key, &metadata)?;
    let reference = repository.map(|id| id.to_string());
    predict_sizes(
        app,
        context,
        workspace,
        model,
        reference.as_deref(),
        (src_lines, src_files, packages),
        fit_id,
    )
}

/// `Predictor.predict`: the probability of success for a leaf of this size in this repository.
fn predict_sizes(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    model: &str,
    repository: Option<&str>,
    (src_lines, src_files, packages): (f64, f64, usize),
    fit_id: Option<&str>,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    check_sizes(src_lines, src_files)?;
    let scope = app.workspace_or_default(workspace)?;
    let requested = fit_id.map(parse_id).and_then(Result::ok);
    let guess = app.runtime.query(
        |session| {
            let registry = session.registry_world(Some(scope))?;
            let by_id = requested
                .map(|id| reads::predictor_fit_by_id(session, id))
                .transpose()?
                .flatten();
            let latest = reads::latest_predictor_fit(session, scope, model)?;
            Ok((registry, by_id, latest))
        },
        |(registry, by_id, latest)| {
            let resolved = predict::resolve_repository(registry, scope, repository);
            let fit = predict::select_fit(model, scope, fit_id, by_id.clone(), latest.clone())?;
            predict::prediction(&fit, &resolved, src_lines, src_files, packages)
        },
    )?;
    Ok(predictor::prediction(&guess))
}

/// `Predictor.predict` / `predict_task`, by what `experiment predict` was asked for.
pub fn experiment_predict(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    model: &str,
    request: &PredictRequest,
    fit_id: Option<&str>,
) -> Result<Value, WgError> {
    match request {
        PredictRequest::Task(task_ref) => {
            predict_task(app, context, workspace, model, task_ref, fit_id)
        }
        PredictRequest::Sizes {
            repository,
            src_lines,
            src_files,
            packages,
        } => predict_sizes(
            app,
            context,
            workspace,
            model,
            Some(repository),
            (*src_lines, *src_files, *packages),
            fit_id,
        ),
    }
}
