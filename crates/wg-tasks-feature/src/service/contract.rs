//! `ContractChecker.check` (`task contract-check`, M28): gathers the task, its tags and criteria,
//! the latest predictor fit of the model and the forecast for the task's declared size, then
//! runs the pure leaf decision of `wg-experiments` and shapes its answer as the verdict dict.

use std::cell::RefCell;

use serde_json::Value as Json;
use wg_experiments::contract::{Fit, Prediction as Forecast, contract_problems, decide};
use wg_experiments::predictor::{forecast_task, repository_ref};
use wg_json::{Value, py_float_repr};
use wg_model::Table;
use wg_model::entity::PredictorFit;
use wg_registry::resolve;
use wg_rows::codec::{id_sql, text_sql};
use wg_rows::sql::{Statement, select_list};
use wg_runtime::{App, RequestContext};
use wg_store::Session;
use wg_store::query::one;
use wg_types::WgError;
use wg_wire::dto::contract_check;

use crate::spec::contract::{prediction_out, verdict_out};

/// `ContractChecker.check`: the verdict for task `reference` against the latest fit of `model`.
/// The task is read across workspaces when no workspace is named; the fit and the repository are
/// read in the default workspace, as `Predictor` does.
pub fn task_contract_check(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    model: &str,
    target: f64,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    if !(0.0..=1.0).contains(&target) {
        return Err(WgError::validation(format!(
            "target must be between 0 and 1, got {}",
            py_float_repr(target)
        ))
        .detail("target", Json::from(target)));
    }
    let task_scope = app.workspace_or_all(workspace)?;
    let fit_scope = app.workspace_or_default(workspace)?;
    let verdict = app.runtime.query(
        |session: &mut Session<'_>| {
            let world = session.task_world(task_scope, &[])?;
            let fit = latest_predictor_fit(session, fit_scope, model)?;
            Ok((world, fit))
        },
        |(world, fit)| {
            let task = resolve::task(&world.tasks, task_scope, reference)?;
            let tags: Vec<String> = world.tasks.tags_of(task.id).map(str::to_owned).collect();
            let criteria: Vec<String> = task.acceptance_criteria.iter().cloned().collect();
            let metadata = Json::Object(task.metadata.clone());
            let problems = contract_problems(&tags, &criteria, &metadata);
            let linked = world
                .tasks
                .repositories_of(task.id)
                .next()
                .map(|link| link.repository_id.to_string());
            let resolved = linked.as_ref().and_then(|text| {
                resolve::repository(&world.tasks.registry, fit_scope, text, false)
                    .ok()
                    .map(|repo| (repo.name.clone(), repo.id.to_string()))
            });
            let repository = repository_ref(linked, resolved);
            let forecast = RefCell::new(None);
            let decided = decide(
                problems,
                || match fit {
                    None => Fit::NoFit,
                    Some(fit) => {
                        let coef: Vec<Json> = fit.coef.iter().cloned().collect();
                        let samples: Vec<Json> = fit.samples.iter().cloned().collect();
                        let repositories: Vec<String> = fit.repositories.iter().cloned().collect();
                        let result = forecast_task(
                            &coef,
                            &samples,
                            &repositories,
                            repository.id.as_deref(),
                            &metadata,
                        )
                        .map(|leaf| {
                            prediction_out(
                                leaf,
                                fit.id,
                                &fit.model,
                                fit.data_cutoff,
                                fit.n,
                                repository.name.clone(),
                            )
                        })
                        .map_err(|err| WgError::workgraph(err.message()));
                        let fit_state = match &result {
                            Ok(out) => Fit::Predicted(Forecast {
                                probability: out.probability,
                                repository_known: out.repository_known,
                            }),
                            Err(_) => Fit::NoFit,
                        };
                        *forecast.borrow_mut() = Some(result);
                        fit_state
                    }
                },
                model,
                target,
            )
            .map_err(WgError::validation)?;
            let predicted = forecast.into_inner().transpose()?;
            Ok(verdict_out(decided, predicted, model, target))
        },
    )?;
    Ok(contract_check::verdict(&verdict))
}

/// The newest fit of `model` in `workspace_id`, if there is one: Python orders by `created_at`
/// descending, `id` descending is the tiebreak. (The experiments feature reads it the same way;
/// it depends on this crate for replay, so the read is repeated here.)
fn latest_predictor_fit(
    session: &mut Session<'_>,
    workspace_id: wg_types::Id,
    model: &str,
) -> Result<Option<PredictorFit>, WgError> {
    one(
        session,
        &Statement {
            sql: format!(
                "SELECT {} FROM \"predictor_fits\" WHERE \"workspace_id\" = ?1 AND \"model\" = ?2 \
                 ORDER BY \"created_at\" DESC, \"id\" DESC LIMIT 1",
                select_list(Table::PredictorFit, None)
            ),
            params: vec![id_sql(workspace_id), text_sql(model)],
        },
    )
}
