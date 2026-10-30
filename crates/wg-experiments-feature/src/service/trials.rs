//! Shell: the trial entry points `trial add`, `trial show` and `trial classify` (IF-20). Each one
//! reads what its decision needs through the session, runs the pure decision in
//! [`crate::spec::trial`] and applies what it returns. Lookups follow `_find` and `_trial` of the
//! Python `ExperimentService`: a reference that is a UUID is an id, anything else a slug (for
//! experiments only).

use wg_json::Value;
use wg_model::entity::{Experiment, ExperimentTrial, Task};
use wg_model::{RowKey, TaskSlice, py_repr_str};
use wg_registry::resolve;
use wg_runtime::spec::app::is_uuid;
use wg_runtime::{App, RequestContext};
use wg_store::Session;
use wg_types::{Id, Permission, WgError, parse_id};

use crate::service::reads;
use crate::spec::trial::{self, AddInput};

/// `ExperimentService.add_trial`: plans one trial of `task` in the running experiment `experiment`.
///
/// # Errors
///
/// Returns the `NotFound` of an unknown experiment or task, and the conflicts and validation
/// failures the decision reports.
pub fn trial_add(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    experiment_ref: &str,
    task_ref: &str,
    input: AddInput<'_>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::ExperimentWrite)?;
    let scope = app.workspace_or_default(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| {
                let experiment = find_experiment(session, scope, experiment_ref)?;
                let tasks = session.task_world(Some(scope), &[])?;
                let next_seq = reads::max_trial_seq(session, experiment.id)?.unwrap_or(0) + 1;
                Ok(AddWorld {
                    experiment,
                    tasks: tasks.tasks,
                    next_seq,
                })
            },
            |world: &AddWorld, stamp| {
                let task = resolve::task(&world.tasks, Some(scope), task_ref)?;
                trial::add(&world.experiment, task, world.next_seq, input, stamp)
            },
            |_, _| Ok(()),
        )?
        .reply)
}

/// What `trial add` reads before it decides.
struct AddWorld {
    experiment: Experiment,
    tasks: TaskSlice,
    next_seq: i64,
}

/// `ExperimentService.trial`: one trial of the workspace, with its task key.
///
/// # Errors
///
/// Returns the `NotFound` of a reference that is not a trial of the workspace.
pub fn trial_show(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    trial_ref: &str,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| load_trial(session, scope, trial_ref),
        |(trial, task_key)| Ok(wg_wire::dto::trial::from_model(trial, task_key)),
    )
}

/// `ExperimentService.classify`: names the cause of a failed or invalid trial.
///
/// # Errors
///
/// Returns the `NotFound` of an unknown trial, and the conflicts, validation failures and
/// rejections the decision reports.
pub fn trial_classify(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    trial_ref: &str,
    cause: &str,
    source: &str,
    notes: Option<&str>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::ExperimentWrite)?;
    let scope = app.workspace_or_default(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| {
                let (trial, task_key) = load_trial(session, scope, trial_ref)?;
                let experiment = experiment_of(session, &trial)?;
                Ok(ClassifyWorld {
                    trial,
                    task_key,
                    experiment,
                })
            },
            |world: &ClassifyWorld, stamp| {
                trial::classify(
                    &world.experiment,
                    &world.trial,
                    &world.task_key,
                    cause,
                    source,
                    notes,
                    stamp,
                )
            },
            |_, _| Ok(()),
        )?
        .reply)
}

/// What `trial classify` reads before it decides.
struct ClassifyWorld {
    trial: ExperimentTrial,
    task_key: String,
    experiment: Experiment,
}

/// `_find`: the experiment of the workspace named by an id or a slug.
fn find_experiment(
    session: &mut Session<'_>,
    workspace_id: Id,
    reference: &str,
) -> Result<Experiment, WgError> {
    let found = if is_uuid(reference) {
        let id = parse_id(reference)?;
        reads::experiment_by_id(session, id)?
    } else {
        reads::experiment_by_slug(session, workspace_id, reference)?
    };
    match found {
        Some(experiment) if experiment.workspace_id == workspace_id => Ok(experiment),
        _ => Err(
            WgError::not_found(format!("experiment {} not found", py_repr_str(reference)))
                .detail("ref", reference),
        ),
    }
}

/// `_trial`: the trial named by `reference` whose experiment is in the workspace, with the key of
/// its task.
pub(crate) fn load_trial(
    session: &mut Session<'_>,
    workspace_id: Id,
    reference: &str,
) -> Result<(ExperimentTrial, String), WgError> {
    let missing = || {
        WgError::not_found(format!("trial {} not found", py_repr_str(reference)))
            .detail("ref", reference)
    };
    if !is_uuid(reference) {
        return Err(missing());
    }
    let trial = reads::trial_by_id(session, parse_id(reference)?)?.ok_or_else(missing)?;
    let experiment = reads::experiment_by_id(session, trial.experiment_id)?.ok_or_else(missing)?;
    if experiment.workspace_id != workspace_id {
        return Err(missing());
    }
    let task_key = task_key(session, trial.task_id)?;
    Ok((trial, task_key))
}

/// The experiment a trial belongs to.
pub(crate) fn experiment_of(
    session: &mut Session<'_>,
    trial: &ExperimentTrial,
) -> Result<Experiment, WgError> {
    reads::experiment_by_id(session, trial.experiment_id)?
        .ok_or_else(|| WgError::not_found(format!("experiment {} not found", trial.experiment_id)))
}

/// The key of the task `task_id`, which the trial output shows.
fn task_key(session: &mut Session<'_>, task_id: Id) -> Result<String, WgError> {
    let task: Option<Task> = session.fetch(&RowKey::Task(task_id))?;
    task.map(|task| task.key.to_string())
        .ok_or_else(|| WgError::not_found(format!("task {task_id} not found")))
}
