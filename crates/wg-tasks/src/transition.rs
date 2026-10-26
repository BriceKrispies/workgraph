//! Applying a status change to a task. Mirrors `_transition`, `_refresh_dependents` and
//! `_refresh_blocked_state` in `core/services/tasks.py`: the dependency gate, the lifecycle
//! check, the timestamps and claim fields a status sets, the version bump, and the events
//! (`task.status_changed`, then `task.reopened` and `task.became_ready` caused by it), followed by
//! the refresh of the tasks that wait on a task that was accepted or reopened.
//!
//! The functions take the slice the change starts from and return a [`Decision`] with the writes
//! and events, the ids still free, and the slice after the writes, so a caller can chain the next
//! change onto it.

use serde_json::{Map, Value};
use wg_model::event::{EventDraft, EventPayload, TaskKeyed, TaskReopened, TaskStatusChanged};
use wg_model::slice::StackSlice;
use wg_model::stamp::{IdSeq, Stamp};
use wg_model::{Decision, TaskSlice, Write};
use wg_types::{ErrorCode, Id, TaskStatus, WgError};

use crate::deps::BLOCKED_BY_DEPENDENCIES;
use crate::lifecycle::check_task_transition;
use crate::promote::{dependency_gate, dependents, refresh_blocked_state};

/// How a status change is requested.
#[derive(Debug, Clone, Copy, Default)]
pub struct Change<'a> {
    pub reason: Option<&'a str>,
    /// The acceptance service's flag: entering `accepted` is allowed.
    pub privileged: bool,
    /// A tracker-driven close.
    pub tracker: bool,
    /// The event that caused the change (`ctx.caused_by`).
    pub caused_by: Option<Id>,
}

/// What a status change produced: the changes, the ids still free and the slice afterwards.
#[derive(Debug, Clone)]
pub struct Applied {
    pub decision: Decision<()>,
    pub ids: IdSeq,
    pub tasks: TaskSlice,
}

/// Python truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
        Value::Number(number) => number.as_f64().is_none_or(|n| n != 0.0),
    }
}

fn missing(task_id: Id) -> WgError {
    WgError::new(ErrorCode::NotFound, format!("task {task_id} not found"))
}

/// Moves `task_id` to `target` (`TaskService._transition`), then refreshes its dependents when
/// the change accepts or reopens it. A change to the status the task already has changes nothing
/// except the `blocked_reason` the gate may set.
pub fn transition(
    tasks: &TaskSlice,
    stacks: &StackSlice,
    stamp: &Stamp,
    ids: IdSeq,
    task_id: Id,
    target: TaskStatus,
    change: Change<'_>,
) -> Result<Applied, WgError> {
    let task = tasks.task(task_id).ok_or_else(|| missing(task_id))?;
    let current = task.status;
    let gated = dependency_gate(tasks, stacks, task, target, change.reason)?;
    let target = gated.target;
    let reason = gated.reason;

    let mut next = task.clone();
    if gated.blocked_by_dependencies {
        next.metadata.insert(
            "blocked_reason".to_owned(),
            Value::String(BLOCKED_BY_DEPENDENCIES.to_owned()),
        );
    }
    if current == target {
        let unchanged = next.metadata == task.metadata;
        let decision = if unchanged {
            Decision::new(())
        } else {
            Decision::new(()).write(Write::update(next))
        };
        let tasks = tasks.apply_all(decision.writes.iter());
        return Ok(Applied {
            decision,
            ids,
            tasks,
        });
    }
    check_task_transition(current, target, change.privileged, change.tracker)?;

    let at = stamp.at;
    next.status = target;
    if target != TaskStatus::Blocked && next.metadata.get("blocked_reason").is_some_and(truthy) {
        next.metadata = next
            .metadata
            .iter()
            .filter(|(key, _)| key.as_str() != "blocked_reason")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Map<String, Value>>();
    }
    if matches!(target, TaskStatus::Claimed | TaskStatus::Running) && next.started_at.is_none() {
        next.started_at = Some(at);
    }
    if matches!(
        target,
        TaskStatus::AwaitingVerification | TaskStatus::AwaitingReview
    ) {
        next.completed_at = Some(at);
    }
    if target == TaskStatus::Accepted {
        next.accepted_at = Some(at);
        if next.completed_at.is_none() {
            next.completed_at = Some(at);
        }
    }
    if !matches!(target, TaskStatus::Claimed | TaskStatus::Running) {
        next.claimed_by = None;
        next.claim_expires_at = None;
    }
    let reopened = matches!(current, TaskStatus::Accepted | TaskStatus::Cancelled);
    if reopened {
        next.accepted_at = None;
    }
    next.version += 1;
    next.updated_at = at;

    let workspace = Some(next.workspace_id);
    let (status_id, ids) = ids.next();
    let mut status_event = EventDraft::new(
        stamp,
        status_id,
        workspace,
        task_id,
        EventPayload::TaskStatusChanged(TaskStatusChanged {
            from: current,
            to: target,
            reason: reason.clone(),
            version: next.version,
            attempt_id: None,
            run_id: None,
            extra: Map::new(),
        }),
    );
    if let Some(cause) = change.caused_by {
        status_event = status_event.caused_by(cause);
    }
    let mut decision = Decision::new(())
        .write(Write::update(next.clone()))
        .event(status_event);
    let mut ids = ids;
    if reopened {
        let (id, after) = ids.next();
        ids = after;
        decision = decision.event(
            EventDraft::new(
                stamp,
                id,
                workspace,
                task_id,
                EventPayload::TaskReopened(TaskReopened {
                    from: current,
                    to: target,
                    reason: reason.clone(),
                }),
            )
            .caused_by(status_id),
        );
    }
    if target == TaskStatus::Ready {
        let (id, after) = ids.next();
        ids = after;
        decision = decision.event(
            EventDraft::new(
                stamp,
                id,
                workspace,
                task_id,
                EventPayload::TaskBecameReady(TaskKeyed {
                    key: next.key.clone(),
                }),
            )
            .caused_by(status_id),
        );
    }
    let mut applied = Applied {
        tasks: tasks.apply_all(decision.writes.iter()),
        decision,
        ids,
    };
    if current == TaskStatus::Accepted || target == TaskStatus::Accepted {
        applied = refresh_dependents(applied, stacks, stamp, task_id, status_id)?;
    }
    Ok(applied)
}

/// Refreshes every task that depends on `task_id` (`_refresh_dependents`), one after the other,
/// each seeing the changes of the ones before it.
fn refresh_dependents(
    start: Applied,
    stacks: &StackSlice,
    stamp: &Stamp,
    task_id: Id,
    caused_by: Id,
) -> Result<Applied, WgError> {
    let waiting: Vec<Id> = dependents(&start.tasks, task_id)
        .into_iter()
        .map(|task| task.id)
        .collect();
    waiting.into_iter().try_fold(start, |so_far, dependent| {
        let Some(task) = so_far.tasks.task(dependent) else {
            return Ok(so_far);
        };
        let Some(refresh) = refresh_blocked_state(&so_far.tasks, stacks, task) else {
            return Ok(so_far);
        };
        let step = transition(
            &so_far.tasks,
            stacks,
            stamp,
            so_far.ids,
            dependent,
            refresh.target,
            Change {
                reason: Some(refresh.reason),
                caused_by: Some(caused_by),
                ..Change::default()
            },
        )?;
        Ok(Applied {
            decision: so_far.decision.then(step.decision),
            ids: step.ids,
            tasks: step.tasks,
        })
    })
}
