//! Task acceptance decisions: `accept`, `accept_manually`, `reject`, `promote_dependents` and the
//! acceptance list (the pure half of `VerificationService`). The pieces stack landing shares live in
//! `wg_tasks::accept`.

use serde_json::Value as Json;
use wg_json::Value;
use wg_model::entity::{TaskAcceptance, VerificationConfig};
use wg_model::event::{EventDraft, EventPayload, StackParentRejected, TaskRejected};
use wg_model::slice::StackSlice;
use wg_model::stamp::Stamp;
use wg_model::{Decision, Write};
use wg_registry::resolve;
use wg_rows::task_world_sql::TaskWorld;
use wg_stacks::landing::{ACCEPTABLE_TASK_STATUSES, guard_child};
use wg_tasks::accept::*;
use wg_tasks::promote::promote_dependents as promotable;
use wg_tasks::transition::{Applied, Change, transition};
use wg_types::pyval;
use wg_types::{Id, Permission, TaskStatus, VerificationStatus, WgError};
use wg_verify::{LatestRun, TaskScope, accept_refusal, applicable_checks, ran_fast_tier_only};
use wg_wire::convention::text;
use wg_wire::dto::verification::acceptance_from_model;
/// What `accept` reads: the task world and the facts of the task named by the reference.
#[derive(Debug, Clone, Default)]
pub struct AcceptWorld {
    pub world: TaskWorld,
    pub configs: Vec<VerificationConfig>,
    pub facts: AcceptFacts,
    /// Integration work for the task's stack is queued or running.
    pub busy: bool,
}

/// Where an `accept` goes.
#[derive(Debug, Clone)]
pub enum Route {
    /// The task is accepted; this is the `AcceptanceOut`. `rebuild` is the stack whose integration
    /// branches the shell rebuilds without the child, when the child's commits were on them.
    Done { out: Value, rebuild: Option<Id> },
    /// The task is the parent of an active stack: the shell lands that stack.
    Land { stack_id: Id },
}

fn scope_of(tasks: &wg_model::TaskSlice, task: &wg_model::Task) -> TaskScope {
    TaskScope {
        workspace_id: task.workspace_id,
        task_id: task.id,
        project_ids: tasks
            .projects_of(task.id)
            .map(|row| row.project_id)
            .collect(),
        repository_ids: tasks
            .repositories_of(task.id)
            .map(|row| row.repository_id)
            .collect(),
    }
}

/// `VerificationService.accept` (automated) and `accept_manually`.
pub fn accept_task(
    loaded: &AcceptWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    manual: bool,
    reason: &str,
) -> Result<Decision<Route>, WgError> {
    stamp.require(&[Permission::TaskAccept])?;
    if manual && reason.trim().is_empty() {
        return Err(WgError::validation(
            "manual acceptance requires a recorded reason",
        ));
    }
    let tasks = &loaded.world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    if let Some(item) = tasks.external_item_of(task.id) {
        return Err(WgError::permission_denied(format!(
            "{} is tracked by {}; the tracker closes it when {} reaches done",
            task.key, item.connection, item.external_key
        ))
        .detail("connection", item.connection.as_str())
        .detail("external_key", item.external_key.as_str()));
    }
    if let Some(stack) = loaded.world.stacks.active_stack_of_parent(task.id) {
        return Ok(Decision::new(Route::Land { stack_id: stack.id }));
    }
    let mode = if manual { "manual" } else { "automated" };
    let reason = if manual || !reason.is_empty() {
        reason
    } else {
        "required checks passed"
    };
    if !manual {
        let checks = applicable_checks(&loaded.configs, &scope_of(tasks, task))?;
        let latest = loaded.facts.run.as_ref().map(|found| LatestRun {
            run_id: found.id,
            status: VerificationStatus::parse(&found.status).unwrap_or(VerificationStatus::Pending),
            required_passed: found.required_passed,
            fast_tier_only: ran_fast_tier_only(
                &checks,
                &found.config_snapshot.iter().cloned().collect::<Vec<_>>(),
            ),
        });
        if let Some(refusal) = accept_refusal(checks.len(), latest) {
            return Err(refusal.to_error(&task.key.to_string()));
        }
    }
    if let Some((stack, _)) = active_item(&loaded.world.stacks, task.id) {
        guard_child(&task.key.to_string(), &stack.id.to_string(), loaded.busy)?;
    }
    let run = Run::start(&loaded.world, stamp);
    let (accepted, acceptance) = accept_in(&run, stamp, task.id, mode, reason, &loaded.facts)?;
    let (released, rebuild) = release_child(&accepted, stamp, task.id)?;
    Ok(released.applied.decision.with_reply(Route::Done {
        out: acceptance_from_model(&acceptance),
        rebuild,
    }))
}

/// The mode and reason a stack landing records for an `accept` of the stack's parent.
pub fn land_request_of(manual: bool, reason: &str) -> (&'static str, String) {
    if manual {
        ("manual", reason.to_owned())
    } else if reason.is_empty() {
        ("automated", "required checks passed".to_owned())
    } else {
        ("automated", reason.to_owned())
    }
}

/// The `manual` flag and the reason of a validated `AcceptBody` or `RejectBody`.
pub fn body_parts(body: &Json) -> (bool, String) {
    (
        pyval::flag(body.get("manual")),
        pyval::text(body.get("reason")).to_owned(),
    )
}

/// The active stack the task is an item of, whose integration its acceptance must not interrupt.
pub fn item_stack_id(stacks: &StackSlice, task: Id) -> Option<Id> {
    active_item(stacks, task).map(|(stack, _)| stack.id)
}

/// `VerificationService.reject`: the task goes back to `ready`, with a recorded decision.
pub fn reject_task(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    reason: &str,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskAccept])?;
    if reason.trim().is_empty() {
        return Err(WgError::validation("rejection requires a reason"));
    }
    let task = resolve::task(&world.tasks, workspace, reference)?;
    if !ACCEPTABLE_TASK_STATUSES.contains(&task.status) {
        return Err(WgError::invalid_transition(format!(
            "{} is {}; nothing to reject",
            task.key, task.status
        )));
    }
    let ([acceptance_id, event_id], ids) = stamp.ids().take::<2>();
    let acceptance = TaskAcceptance {
        id: acceptance_id,
        task_id: task.id,
        attempt_id: None,
        verification_run_id: None,
        decision: "rejected".to_owned(),
        mode: "manual".to_owned(),
        reason: reason.to_owned(),
        actor: stamp.actor.to_string(),
        manifest_id: None,
        created_at: stamp.at,
    };
    let rejected = EventDraft::new(
        stamp,
        event_id,
        Some(task.workspace_id),
        task.id,
        EventPayload::TaskRejected(TaskRejected {
            reason: reason.to_owned(),
        }),
    );
    let recorded = Decision::new(())
        .write(Write::insert(acceptance.clone()))
        .event(rejected);
    let moved = transition(
        &world.tasks,
        &world.stacks,
        stamp,
        ids,
        task.id,
        TaskStatus::Ready,
        Change {
            reason: Some(&format!("rejected: {reason}")),
            caused_by: Some(event_id),
            ..Change::default()
        },
    )?;
    let noted = match world.stacks.active_stack_of_parent(task.id) {
        Some(stack) => {
            let (stack_event, _) = moved.ids.next();
            Decision::new(()).event(EventDraft::new(
                stamp,
                stack_event,
                Some(stack.workspace_id),
                stack.id,
                EventPayload::StackParentRejected(StackParentRejected {
                    task_key: task.key.clone(),
                    reason: reason.to_owned(),
                }),
            ))
        }
        None => Decision::new(()),
    };
    Ok(recorded
        .then(moved.decision)
        .then(noted)
        .with_reply(acceptance_from_model(&acceptance)))
}

/// `TaskService.promote_dependents`: the backlog dependents of the task whose dependencies are
/// now all accepted go to `ready`; the reply is their keys.
pub fn promote_dependents(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let task = resolve::task(&world.tasks, workspace, reference)?;
    let moved = promotable(&world.tasks, &world.stacks, task.id);
    let done = moved.iter().try_fold(
        Applied {
            decision: Decision::new(()),
            ids: stamp.ids(),
            tasks: world.tasks.clone(),
        },
        |so_far, dependent| {
            let next = transition(
                &so_far.tasks,
                &world.stacks,
                stamp,
                so_far.ids,
                dependent.id,
                TaskStatus::Ready,
                Change {
                    reason: Some("dependencies accepted"),
                    ..Change::default()
                },
            )?;
            Ok::<_, WgError>(Applied {
                decision: so_far.decision.then(next.decision),
                ids: next.ids,
                tasks: next.tasks,
            })
        },
    )?;
    let keys = moved
        .iter()
        .map(|dependent| text(&dependent.key.to_string()))
        .collect();
    Ok(done.decision.with_reply(Value::List(keys)))
}

/// `VerificationService.acceptances`: the task's decisions, oldest first.
pub fn acceptances_out(rows: &[TaskAcceptance]) -> Value {
    let mut ordered: Vec<&TaskAcceptance> = rows.iter().collect();
    ordered.sort_by_key(|row| (row.created_at, row.id));
    Value::List(ordered.into_iter().map(acceptance_from_model).collect())
}

/// `{**to_jsonable(out), "promoted": promoted}` for `accept --promote-dependents`: the model
/// as the plain dict `model_dump(mode="json")` makes of it (datetimes as `Z` text), with the
/// promoted keys appended.
pub fn with_promoted(out: &Value, promoted: &Value) -> Value {
    let plain = |value: &Value| match value {
        Value::DateTime(at) => Value::Str(wg_json::PyStr::from(at.format_z())),
        other => other.clone(),
    };
    match out {
        Value::Model { fields, .. } => Value::Dict(
            fields
                .iter()
                .map(|(name, value)| (wg_json::PyStr::from(name.as_str()), plain(value)))
                .chain(std::iter::once((
                    wg_json::PyStr::from("promoted"),
                    promoted.clone(),
                )))
                .collect(),
        ),
        other => other.clone(),
    }
}
