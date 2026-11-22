//! `task intake`: the triage of proposed tasks. `list` shows them per repository; `admit`, `merge`
//! and `drop` each take one or more keys and act on every key in its own transaction, reporting
//! the keys that failed (a task that is not proposed, a missing target) without stopping the rest.
//! Each action goes through the task services, so permissions, transitions, events and the
//! dependency gate are the services' own.

use serde_json::{Value as Json, json};
use wg_json::Value;
use wg_model::Task;
use wg_registry::resolve;
use wg_runtime::{App, RequestContext};
use wg_tasks::intake::{
    admit_status, dropped_reason, merge_comment, merged_reason, require_proposed,
};
use wg_types::{TaskStatus, WgError};

use super::{task_add_relation, task_comment, task_update};
use crate::spec::intake::{action_report, proposed_by_repository};

/// `task intake list`: the proposed tasks of the workspace, per repository.
pub fn intake_list(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    repository: Option<&str>,
    project: Option<&str>,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    let listing = app.runtime.query(
        |session| session.task_world(Some(scope), &[]),
        |world| proposed_by_repository(world, scope, repository, project),
    )?;
    Ok(wg_wire::convention::json_value(&listing))
}

/// The task `reference` names (its workspace id and the row), when it is proposed.
fn proposal(
    app: &mut App,
    workspace: Option<&str>,
    reference: &str,
) -> Result<(String, Task), WgError> {
    let scope = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| session.task_world(Some(scope), &[]),
        |world| {
            let task = resolve::task(&world.tasks, Some(scope), reference)?;
            require_proposed(task)?;
            Ok((task.workspace_id.to_string(), task.clone()))
        },
    )
}

/// `task intake admit`: proposals to backlog, or to ready with `ready`.
pub fn intake_admit(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    keys: &[String],
    ready: bool,
) -> Result<Value, WgError> {
    let target = admit_status(ready);
    let results = keys
        .iter()
        .map(|key| {
            let done = proposal(app, workspace, key).and_then(|(ws, task)| {
                task_update(
                    app,
                    context,
                    Some(&ws),
                    &task.key.to_string(),
                    &json!({"status": target.as_str(), "reason": "admitted at intake"}),
                )?;
                Ok(json!({"status": target.as_str()}))
            });
            (key.clone(), done)
        })
        .collect();
    Ok(action_report("admit", results))
}

/// `task intake merge`: each proposal's description and criteria become a comment on `into`, the
/// proposal is marked as a duplicate of it and cancelled.
pub fn intake_merge(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    keys: &[String],
    into: &str,
) -> Result<Value, WgError> {
    let results = keys
        .iter()
        .map(|key| (key.clone(), merge_one(app, context, workspace, key, into)))
        .collect();
    Ok(action_report("merge", results))
}

fn merge_one(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    key: &str,
    into: &str,
) -> Result<Json, WgError> {
    let (ws, task) = proposal(app, workspace, key)?;
    let scope = app.workspace_or_default(workspace)?;
    let target_key = app.runtime.query(
        |session| session.task_world(Some(scope), &[]),
        |world| {
            let target = resolve::task(&world.tasks, Some(scope), into)?;
            if target.id == task.id {
                return Err(WgError::validation(format!(
                    "{} cannot be merged into itself",
                    task.key
                )));
            }
            if matches!(target.status, TaskStatus::Accepted | TaskStatus::Cancelled) {
                return Err(WgError::invalid_transition(format!(
                    "task {} is {}; merge into an open task",
                    target.key, target.status
                )));
            }
            Ok(target.key.to_string())
        },
    )?;
    let source_key = task.key.to_string();
    task_comment(
        app,
        context,
        Some(&ws),
        &target_key,
        &json!({"body": merge_comment(&task)}),
    )?;
    task_add_relation(
        app,
        context,
        Some(&ws),
        &source_key,
        &json!({"target": target_key, "relation_type": "duplicates"}),
    )?;
    task_update(
        app,
        context,
        Some(&ws),
        &source_key,
        &json!({"status": "cancelled", "reason": merged_reason(&target_key)}),
    )?;
    Ok(json!({"merged_into": target_key}))
}

/// `task intake drop`: proposals cancelled, with the reason as a comment.
pub fn intake_drop(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    keys: &[String],
    reason: &str,
) -> Result<Value, WgError> {
    if reason.trim().is_empty() {
        return Err(WgError::validation("a drop needs a reason"));
    }
    let results = keys
        .iter()
        .map(|key| {
            let done = proposal(app, workspace, key).and_then(|(ws, task)| {
                let key = task.key.to_string();
                task_comment(
                    app,
                    context,
                    Some(&ws),
                    &key,
                    &json!({"body": dropped_reason(reason)}),
                )?;
                task_update(
                    app,
                    context,
                    Some(&ws),
                    &key,
                    &json!({"status": "cancelled", "reason": dropped_reason(reason)}),
                )?;
                Ok(json!({"status": "cancelled"}))
            });
            (key.clone(), done)
        })
        .collect();
    Ok(action_report("drop", results))
}
