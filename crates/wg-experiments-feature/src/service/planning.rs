//! The trial planner entry points of the application (IF-19): `experiment next` and
//! `experiment report`. Both read an experiment with its trials in one snapshot; `next` then
//! adds the trial the plan chose in its own transaction, as the Python planner does.

use std::collections::BTreeMap;

use wg_json::Value;
use wg_model::RowKey;
use wg_model::entity::SubagentRun;
use wg_runtime::{App, RequestContext};
use wg_types::{Permission, WgError};

use super::experiments::find;
use super::reads;
use crate::spec::planning::{self, Planned, RunUsage};

/// `TrialPlanner.next`: the pending trial, a newly created one, or why the experiment is done.
pub fn experiment_next(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::ExperimentWrite)?;
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    let (experiment, trials) = app.runtime.query(
        |session| {
            let found = find(session, scope, reference)?;
            let trials = reads::trials_with_task_keys(session, found.id)?;
            Ok((found, trials))
        },
        |loaded| Ok(loaded.clone()),
    )?;
    let plan = planning::plan_next(&experiment.protocol, &trials, || {
        app.runtime.query(
            |session| session.task_world(Some(scope), &[]),
            |world| planning::pool_tasks(&world.tasks, scope, &experiment.protocol),
        )
    })?;
    let added = match &plan.planned {
        Planned::Add(addition) => Some(
            app.runtime
                .decide(
                    context,
                    |session| {
                        let found = find(session, scope, reference)?;
                        let max_seq = reads::max_trial_seq(session, found.id)?;
                        Ok((found, max_seq))
                    },
                    |(found, max_seq), stamp| {
                        planning::planned_trial(stamp, found, addition, *max_seq)
                    },
                )?
                .reply,
        ),
        _ => None,
    };
    Ok(planning::next_value(&plan, &trials, added.as_ref()))
}

/// `TrialPlanner.report`: per-arm measurements of an experiment's trials.
pub fn experiment_report(
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
            let runs = planning::run_ids(&trials)
                .into_iter()
                .map(|id| {
                    session
                        .fetch::<SubagentRun>(&RowKey::SubagentRun(id))
                        .map(|run| run.map(|run| (id, RunUsage::of(&run))))
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<BTreeMap<_, _>>();
            Ok((found, trials, runs))
        },
        |(found, trials, runs)| planning::report_value(found, trials, runs),
    )
}
