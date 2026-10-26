//! The decision entry points of the application: the services behind `decision list|show|request|
//! choose|withdraw`, the `/decisions` routes and the decision tools (`DecisionService`).
//!
//! Each one resolves the workspace, loads what the call reads in the transaction of the call,
//! hands it to the pure functions of [`crate::spec`] and [`crate::core`] and applies what they
//! return. The writes keep the full-text index of the tasks they touch in step, as the task entry
//! points do. These are free functions over `&mut App` (and `&mut Session` in [`reads`]): the
//! feature's Shell, reached from the `cli`, `http` and `mcp` tables.

pub mod reads;

use serde_json::Value as Json;
use wg_json::Value;
use wg_model::RowKey;
use wg_model::entity::TaskDecision;
use wg_registry::resolve;
use wg_runtime::{App, RequestContext, index_written};
use wg_store::Session;
use wg_types::{ErrorCode, Id, Permission, WgError};

use crate::core::Answer;
use crate::spec::{self as spec, DecisionFilter, DecisionWorld};

/// The decision `reference` names in the workspace `scope` (any workspace when `None`), with the
/// task world of its workspace and the decisions on its task.
fn load_decision(
    session: &mut Session<'_>,
    scope: Option<Id>,
    reference: &str,
) -> Result<DecisionWorld, WgError> {
    let found = match spec::decision_key(reference) {
        Some(id) => session.fetch::<TaskDecision>(&RowKey::TaskDecision(id))?,
        None => None,
    };
    let decision = spec::locate(found, scope, reference)?;
    Ok(DecisionWorld {
        world: session.task_world(Some(decision.workspace_id), &[])?,
        siblings: reads::decisions_for_task(session, decision.task_id)?,
        decision,
    })
}

/// `DecisionService.request`: opens a decision on a task and tags the task `needs-decision`.
pub fn request(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task: &str,
    question: &str,
    options: &[Json],
) -> Result<Value, WgError> {
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(scope, &[]),
            |world, stamp| spec::request(world, stamp, scope, task, question, options),
            index_written,
        )?
        .reply)
}

/// `DecisionService.list`: decisions with their task.
pub fn list(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    filter: DecisionFilter,
    limit: i64,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| reads::decisions_by_status_with_task(session, scope, filter, limit),
        |rows| Ok(spec::list_out(rows)),
    )
}

/// `DecisionService.get`: one decision with its task.
pub fn get(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    decision_id: &str,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_all(workspace)?;
    app.runtime.query(
        |session| {
            let found = match spec::decision_key(decision_id) {
                Some(id) => session.fetch::<TaskDecision>(&RowKey::TaskDecision(id))?,
                None => None,
            };
            let row = spec::locate(found, scope, decision_id)?;
            reads::decision_by_id_with_task(session, row.workspace_id, row.id)
        },
        |row| {
            row.as_ref().map(spec::with_task_out).ok_or_else(|| {
                WgError::new(ErrorCode::WorkgraphError, "the decision's task vanished")
            })
        },
    )
}

/// `DecisionService.for_task`: every decision on a task, oldest first.
pub fn for_task(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task: &str,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_all(workspace)?;
    app.runtime.query(
        |session| {
            let world = session.task_world(scope, &[])?;
            let id = resolve::task(&world.tasks, scope, task)?.id;
            let key = world
                .tasks
                .task(id)
                .map(|found| found.key.to_string())
                .unwrap_or_default();
            Ok((key, reads::decisions_for_task(session, id)?))
        },
        |(key, rows)| Ok(spec::for_task_out(key, rows)),
    )
}

/// `DecisionService.choose`: answers a decision with an option or free text.
pub fn choose(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    decision_id: &str,
    option_id: Option<&str>,
    other_text: Option<&str>,
    note: Option<&str>,
) -> Result<Value, WgError> {
    let answer = Answer {
        option_id,
        other_text,
        note,
    };
    let stamp = app.runtime.stamps.stamp(context)?;
    spec::check_choose(&stamp, option_id, other_text)?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| load_decision(session, scope, decision_id),
            |load, stamp| spec::choose(load, stamp, answer),
            index_written,
        )?
        .reply)
}

/// `DecisionService.withdraw`: retires an open decision.
pub fn withdraw(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    decision_id: &str,
    reason: Option<&str>,
) -> Result<Value, WgError> {
    let stamp = app.runtime.stamps.stamp(context)?;
    stamp.require(&[Permission::TaskWrite])?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| load_decision(session, scope, decision_id),
            |load, stamp| spec::withdraw(load, stamp, reason),
            index_written,
        )?
        .reply)
}
