//! The acceptance entry points of the application (IF-24): `task accept`, `task reject`,
//! `task promote-dependents` and the acceptance list. Each one loads what the decision reads,
//! runs it in the transaction of the call and applies the route it returns. A route that lands
//! a stack is run by the stacks feature (`wg_stacks_feature::service::exec`).

use crate::spec::acceptance::{
    AcceptWorld, Route, accept_task, acceptances_out, item_stack_id, land_request_of,
    promote_dependents, reject_task, with_promoted,
};
use wg_json::Value;
use wg_model::entity::AgentAttempt;
use wg_registry::resolve;
use wg_runtime::{App, RequestContext, index_written};
use wg_stacks_feature::service::exec::{integration_busy, land_stack_accepted};
use wg_store::{Session, TaskWorld};
use wg_tasks::accept::AcceptFacts;
use wg_types::{AttemptStatus, Id, Permission, WgError};

/// The latest succeeded attempt of `task` (`_latest_succeeded_attempt`).
fn latest_succeeded(session: &mut Session<'_>, task: Id) -> Result<Option<AgentAttempt>, WgError> {
    Ok(session
        .attempts_of_task(task, &[AttemptStatus::Succeeded])?
        .into_iter()
        .max_by_key(|attempt| attempt.number))
}

fn facts_of(session: &mut Session<'_>, task: Id) -> Result<AcceptFacts, WgError> {
    let run = session.latest_verification_run(task)?;
    let manifest = match run.as_ref().and_then(|found| found.manifest_id) {
        Some(id) => session.manifest_by_id(id)?,
        None => None,
    };
    Ok(AcceptFacts {
        run,
        manifest,
        attempt: latest_succeeded(session, task)?,
    })
}

/// What `accept` reads for the task `reference` names.
fn load_accept(
    session: &mut Session<'_>,
    scope: Option<Id>,
    reference: &str,
) -> Result<AcceptWorld, WgError> {
    let world = session.task_world(scope, &[])?;
    let Some((task, workspace)) = resolve::task(&world.tasks, scope, reference)
        .ok()
        .map(|found| (found.id, found.workspace_id))
    else {
        return Ok(AcceptWorld {
            world,
            ..AcceptWorld::default()
        });
    };
    let busy = match item_stack_id(&world.stacks, task) {
        Some(stack) => integration_busy(session, stack)?,
        None => false,
    };
    Ok(AcceptWorld {
        configs: session.verification_configs(workspace)?,
        facts: facts_of(session, task)?,
        busy,
        world,
    })
}

/// `VerificationService.accept`: automated acceptance. The parent of a stack lands the
/// stack instead.
pub fn task_accept(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    reason: &str,
) -> Result<Value, WgError> {
    accept_with(app, context, workspace, reference, false, reason)
}

/// `VerificationService.accept_manually`: the acceptance of a reviewer, with a reason.
pub fn task_accept_manually(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    reason: &str,
) -> Result<Value, WgError> {
    accept_with(app, context, workspace, reference, true, reason)
}

fn accept_with(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    manual: bool,
    reason: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskAccept)?;
    let scope = app.workspace_or_all(workspace)?;
    let route = app
        .runtime
        .decide_then(
            context,
            |session| load_accept(session, scope, reference),
            |loaded: &AcceptWorld, stamp| {
                accept_task(loaded, stamp, scope, reference, manual, reason)
            },
            index_written,
        )?
        .reply;
    match route {
        Route::Done(out) => Ok(out),
        Route::Land { stack_id } => {
            let (mode, text) = land_request_of(manual, reason);
            land_stack_accepted(app, context, stack_id, mode, &text, true)
        }
    }
}

/// `VerificationService.reject`.
pub fn task_reject(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    reason: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskAccept)?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(scope, &[]),
            |world: &TaskWorld, stamp| reject_task(world, stamp, scope, reference, reason),
            index_written,
        )?
        .reply)
}

/// `TaskService.promote_dependents`: the keys of the dependents moved to `ready`.
pub fn task_promote_dependents(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskWrite)?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(scope, &[]),
            |world: &TaskWorld, stamp| promote_dependents(world, stamp, scope, reference),
            index_written,
        )?
        .reply)
}

/// `accept --promote-dependents`: the acceptance and the keys it promoted.
pub fn task_accept_promoting(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    manual: bool,
    reason: &str,
) -> Result<Value, WgError> {
    let out = accept_with(app, context, workspace, reference, manual, reason)?;
    let promoted = task_promote_dependents(app, context, workspace, reference)?;
    Ok(with_promoted(&out, &promoted))
}

/// `VerificationService.acceptances`: the decisions recorded for a task, oldest first.
pub fn task_acceptances(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_all(workspace)?;
    app.runtime.query(
        |session| {
            let world = session.task_world(scope, &[])?;
            let task = resolve::task(&world.tasks, scope, reference)?.id;
            session.task_acceptances(task)
        },
        |rows| Ok(acceptances_out(rows)),
    )
}
