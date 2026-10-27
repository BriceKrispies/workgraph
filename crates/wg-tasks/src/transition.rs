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
use wg_model::Task;
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
    /// The `attempt_id` key of the status event (`extra={"attempt_id": ...}`): absent, or written
    /// as the id or as null.
    pub attempt_id: Option<Option<Id>>,
    /// The `run_id` key of the status event (`extra={"run_id": ...}`): absent, or written as the
    /// id or as null.
    pub run_id: Option<Option<Id>>,
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

/// The metadata with `key` set to `value`.
fn with_entry(metadata: &Map<String, Value>, key: &str, value: Value) -> Map<String, Value> {
    metadata
        .iter()
        .filter(|(name, _)| name.as_str() != key)
        .map(|(name, member)| (name.clone(), member.clone()))
        .chain(std::iter::once((key.to_owned(), value)))
        .collect()
}

/// The metadata without `key`.
fn without_entry(metadata: &Map<String, Value>, key: &str) -> Map<String, Value> {
    metadata
        .iter()
        .filter(|(name, _)| name.as_str() != key)
        .map(|(name, member)| (name.clone(), member.clone()))
        .collect()
}

/// The task as the change leaves it, before its events.
fn moved(
    task: &Task,
    metadata: Map<String, Value>,
    current: TaskStatus,
    target: TaskStatus,
    at: wg_types::Timestamp,
) -> Task {
    let holds_claim = matches!(target, TaskStatus::Claimed | TaskStatus::Running);
    let awaiting = matches!(
        target,
        TaskStatus::AwaitingVerification | TaskStatus::AwaitingReview
    );
    let accepted = target == TaskStatus::Accepted;
    let reopened = matches!(current, TaskStatus::Accepted | TaskStatus::Cancelled);
    Task {
        status: target,
        metadata: if target != TaskStatus::Blocked
            && metadata.get("blocked_reason").is_some_and(truthy)
        {
            without_entry(&metadata, "blocked_reason")
        } else {
            metadata
        },
        started_at: if holds_claim && task.started_at.is_none() {
            Some(at)
        } else {
            task.started_at
        },
        completed_at: if awaiting || (accepted && task.completed_at.is_none()) {
            Some(at)
        } else {
            task.completed_at
        },
        accepted_at: if reopened {
            None
        } else if accepted {
            Some(at)
        } else {
            task.accepted_at
        },
        claimed_by: if holds_claim {
            task.claimed_by.clone()
        } else {
            None
        },
        claim_expires_at: if holds_claim {
            task.claim_expires_at
        } else {
            None
        },
        version: task.version + 1,
        updated_at: at,
        ..task.clone()
    }
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
    let metadata = if gated.blocked_by_dependencies {
        with_entry(
            &task.metadata,
            "blocked_reason",
            Value::String(BLOCKED_BY_DEPENDENCIES.to_owned()),
        )
    } else {
        task.metadata.clone()
    };
    if current == target {
        let decision = if metadata == task.metadata {
            Decision::new(())
        } else {
            Decision::new(()).write(Write::update(Task {
                metadata,
                ..task.clone()
            }))
        };
        return Ok(Applied {
            tasks: tasks.apply_all(decision.writes.iter()),
            decision,
            ids,
        });
    }
    check_task_transition(current, target, change.privileged, change.tracker)?;

    let next = moved(task, metadata, current, target, stamp.at);
    let reopened = matches!(current, TaskStatus::Accepted | TaskStatus::Cancelled);
    let workspace = Some(next.workspace_id);
    let (status_id, ids) = ids.next();
    let (reopened_id, ids) = if reopened {
        ids.next()
    } else {
        (status_id, ids)
    };
    let (ready_id, ids) = if target == TaskStatus::Ready {
        ids.next()
    } else {
        (status_id, ids)
    };
    let status_event = EventDraft::new(
        stamp,
        status_id,
        workspace,
        task_id,
        EventPayload::TaskStatusChanged(TaskStatusChanged {
            from: current,
            to: target,
            reason: gated.reason.clone(),
            version: next.version,
            attempt_id: change.attempt_id,
            run_id: change.run_id,
            extra: Map::new(),
        }),
    );
    let status_event = match change.caused_by {
        Some(cause) => status_event.caused_by(cause),
        None => status_event,
    };
    let reopened_event = reopened.then(|| {
        EventDraft::new(
            stamp,
            reopened_id,
            workspace,
            task_id,
            EventPayload::TaskReopened(TaskReopened {
                from: current,
                to: target,
                reason: gated.reason.clone(),
            }),
        )
        .caused_by(status_id)
    });
    let ready_event = (target == TaskStatus::Ready).then(|| {
        EventDraft::new(
            stamp,
            ready_id,
            workspace,
            task_id,
            EventPayload::TaskBecameReady(TaskKeyed {
                key: next.key.clone(),
            }),
        )
        .caused_by(status_id)
    });
    let decision = std::iter::once(status_event)
        .chain(reopened_event)
        .chain(ready_event)
        .fold(
            Decision::new(()).write(Write::update(next)),
            Decision::event,
        );
    let applied = Applied {
        tasks: tasks.apply_all(decision.writes.iter()),
        decision,
        ids,
    };
    if current == TaskStatus::Accepted || target == TaskStatus::Accepted {
        refresh_dependents(applied, stacks, stamp, task_id, status_id)
    } else {
        Ok(applied)
    }
}

/// Refreshes every task that depends on `task_id` (`_refresh_dependents`), one after the other,
/// each seeing the changes of the ones before it.
pub fn refresh_dependents(
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
        refresh_one(so_far, stacks, stamp, dependent, caused_by)
    })
}

/// Refreshes one task whose dependencies changed (`_refresh_blocked_state`): a blocked task
/// parked by dependencies whose blockers are gone becomes ready, a ready task with blockers is
/// blocked again, any other task is left alone.
pub fn refresh_one(
    start: Applied,
    stacks: &StackSlice,
    stamp: &Stamp,
    task_id: Id,
    caused_by: Id,
) -> Result<Applied, WgError> {
    let Some(task) = start.tasks.task(task_id) else {
        return Ok(start);
    };
    let Some(refresh) = refresh_blocked_state(&start.tasks, stacks, task) else {
        return Ok(start);
    };
    let step = transition(
        &start.tasks,
        stacks,
        stamp,
        start.ids,
        task_id,
        refresh.target,
        Change {
            reason: Some(refresh.reason),
            caused_by: Some(caused_by),
            ..Change::default()
        },
    )?;
    Ok(Applied {
        decision: start.decision.then(step.decision),
        ids: step.ids,
        tasks: step.tasks,
    })
}
