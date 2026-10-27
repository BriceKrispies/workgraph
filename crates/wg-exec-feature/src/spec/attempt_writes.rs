//! The chained half of the attempt writes (`core/services/attempts.py` and
//! `orchestration/runs.py`): a new attempt, a status change and a finish added to a chain of
//! changes, the task moves that follow an attempt and the escalation of a task whose attempts keep
//! failing.
//!
//! The rows and events themselves are [`crate::spec::attempts`]'s (`new_attempt`, `set_status`,
//! `finish`), shared with the run flow; this module chains them onto an [`Applied`]: the decision,
//! the ids still free and the task slice after the writes, so a caller composes the steps of one
//! Python session into one decision. The shell (`service::attempt_runs`) reads the rows, asks here
//! what to write and applies the answer.

use serde_json::{Map, Value, json};
use wg_exec::attempt::{counts_as_failure, escalates};
use wg_exec::manifest::ExecutionManifest;
use wg_model::Decision;
use wg_model::TaskSlice;
use wg_model::entity::{AgentAttempt, TaskTag};
use wg_model::event::{EventDraft, EventPayload, TaskEscalated};
use wg_model::slice::StackSlice;
use wg_model::stamp::Stamp;
use wg_model::{Task, Write};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::transition::{Applied, Change, transition};
use wg_tasks_feature::spec::bodies::ClaimRead;
use wg_tasks_feature::spec::claims::claim_task;
use wg_types::{AttemptStatus, ErrorCode, Id, TaskStatus, WgError};

use crate::spec::attempts;

pub use crate::spec::attempts::Finish;

/// The longest reason a task transition records (`reason[:200]`).
pub const REASON_LIMIT: usize = 200;

/// What a new attempt is made of (the arguments of `new_attempt`).
#[derive(Debug, Clone)]
pub struct NewAttempt {
    pub provider: String,
    pub adapter: String,
    pub status: AttemptStatus,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub parent_attempt_id: Option<Id>,
    pub config_snapshot: Map<String, Value>,
}

/// `new_attempt`: the attempt row for `task` and its `attempt.created` event. `number` is the
/// task's next attempt number.
pub fn created(
    start: Applied,
    stamp: &Stamp,
    task: &Task,
    number: i64,
    spec: NewAttempt,
) -> (Applied, AgentAttempt) {
    let session_id = spec.session_id.clone();
    let (attempt, event, ids) = attempts::new_attempt(
        stamp,
        start.ids,
        task,
        attempts::NewAttempt {
            number,
            status: spec.status,
            provider: spec.provider,
            adapter: spec.adapter,
            agent_id: spec.agent_id,
            model: spec.model,
            parent_attempt_id: spec.parent_attempt_id,
            retry_of_attempt_id: None,
            rerun_of_attempt_id: None,
            config_snapshot: spec.config_snapshot,
            telemetry: Map::new(),
        },
    );
    let attempt = AgentAttempt {
        session_id,
        ..attempt
    };
    let chain = Applied {
        decision: start
            .decision
            .write(Write::insert(attempt.clone()))
            .event(event),
        ids,
        tasks: start.tasks,
    };
    (chain, attempt)
}

/// `set_attempt_status`: the attempt moved to `status` with an `attempt.status_changed` event. A
/// status the attempt already has changes nothing. Entering `running` for the first time sets
/// `started_at` and the time spent queued.
pub fn status_set(
    start: Applied,
    stamp: &Stamp,
    attempt: &AgentAttempt,
    status: AttemptStatus,
) -> Result<(Applied, AgentAttempt), WgError> {
    let (event_id, ids) = start.ids.next();
    let Some((moved, event)) = attempts::set_status(stamp, event_id, attempt, status, Map::new())?
    else {
        return Ok((start, attempt.clone()));
    };
    let chain = Applied {
        decision: start
            .decision
            .write(Write::update(moved.clone()))
            .event(event),
        ids,
        tasks: start.tasks,
    };
    Ok((chain, moved))
}

/// `finish_attempt`: the attempt finished as `status` with its terminal event. An attempt that
/// already finished with this status is left alone.
pub fn finished(
    start: Applied,
    stamp: &Stamp,
    attempt: &AgentAttempt,
    status: AttemptStatus,
    finish: Finish,
) -> Result<(Applied, AgentAttempt), WgError> {
    let (event_id, ids) = start.ids.next();
    let Some((row, draft)) = attempts::finish(stamp, event_id, attempt, status, &finish)? else {
        return Ok((start, attempt.clone()));
    };
    let chain = Applied {
        decision: start
            .decision
            .write(Write::update(row.clone()))
            .event(draft),
        ids,
        tasks: start.tasks,
    };
    Ok((chain, row))
}

/// `TaskService._transition` chained onto `start`: the task moved to `target`.
pub fn task_moved(
    start: Applied,
    stacks: &StackSlice,
    stamp: &Stamp,
    task_id: Id,
    target: TaskStatus,
    reason: &str,
) -> Result<Applied, WgError> {
    let next = transition(
        &start.tasks,
        stacks,
        stamp,
        start.ids,
        task_id,
        target,
        Change {
            reason: Some(reason),
            ..Change::default()
        },
    )?;
    Ok(Applied {
        decision: start.decision.then(next.decision),
        ids: next.ids,
        tasks: next.tasks,
    })
}

/// `_maybe_escalate`: after too many failed attempts the task is flagged for human review
/// instead of retried. `failures` counts the task's failed and timed-out attempts.
pub fn escalated(start: Applied, stamp: &Stamp, task_id: Id, failures: u64, limit: u64) -> Applied {
    let Some(task) = start.tasks.task(task_id) else {
        return start;
    };
    let flagged = start
        .tasks
        .tags_of(task_id)
        .any(|tag| tag == wg_exec::attempt::NEEDS_REVIEW_TAG);
    if !escalates(task.status, failures, limit, flagged) {
        return start;
    }
    let (event_id, ids) = start.ids.next();
    let tag = Write::insert(TaskTag {
        task_id,
        tag: wg_exec::attempt::NEEDS_REVIEW_TAG.to_owned(),
    });
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(task.workspace_id),
        task_id,
        EventPayload::TaskEscalated(TaskEscalated {
            failed_attempts: i64::try_from(failures).unwrap_or(i64::MAX),
            limit: i64::try_from(limit).unwrap_or(i64::MAX),
            reason: "attempt limit reached".to_owned(),
        }),
    );
    let tasks = start.tasks.apply(&tag);
    Applied {
        decision: start.decision.write(tag).event(event),
        ids,
        tasks,
    }
}

/// An empty chain over the task slice, with the stamp's ids.
#[must_use]
pub fn start_chain(stamp: &Stamp, tasks: &TaskSlice) -> Applied {
    Applied {
        decision: Decision::new(()),
        ids: stamp.ids(),
        tasks: tasks.clone(),
    }
}

/// A `validation_failed` error.
pub fn validation(message: impl Into<String>) -> WgError {
    WgError::new(ErrorCode::ValidationFailed, message)
}

/// A task that the slice should hold but does not.
#[must_use]
pub fn gone(task: Id) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("task {task} vanished from the slice"),
    )
}

/// The failed and timed-out attempts of a task, counting `row` in place of its stored version.
#[must_use]
pub fn failures(attempts: &[AgentAttempt], row: &AgentAttempt) -> u64 {
    attempts
        .iter()
        .map(|other| if other.id == row.id { row } else { other })
        .filter(|other| counts_as_failure(other.status))
        .count() as u64
}

/// `execution.max_attempts_per_task` of a configuration document.
#[must_use]
pub fn attempt_limit(config: &Map<String, Value>) -> u64 {
    config
        .get("execution")
        .and_then(|execution| execution.get("max_attempts_per_task"))
        .and_then(Value::as_u64)
        .unwrap_or(3)
}

/// How many ids the claim may draw from a decision's sequence; the attempt's ids follow them.
const CLAIM_IDS: usize = 16;

/// `TaskService._claim_in_session` with a default `ClaimRequest`, as the first step of a decision.
pub fn claimed_chain(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Id,
    task: Id,
) -> Result<Applied, WgError> {
    let read = ClaimRead {
        agent: None,
        lease_minutes: 120,
        expected_version: None,
    };
    let claim = claim_task(world, stamp, Some(workspace), &task.to_string(), &read)?;
    let tasks = world.tasks.apply_all(claim.writes.iter());
    let ids = (0..CLAIM_IDS).fold(stamp.ids(), |ids, _| ids.next().1);
    Ok(Applied {
        decision: claim.with_reply(()),
        ids,
        tasks,
    })
}

/// `task.claimed_by = task.claimed_by or ctx.actor`.
#[must_use]
pub fn claimed(chain: Applied, task_id: Id, actor: &str) -> Applied {
    match chain.tasks.task(task_id) {
        Some(task) if task.claimed_by.is_none() => {
            let write = Write::update(Task {
                claimed_by: Some(actor.to_owned()),
                ..task.clone()
            });
            Applied {
                tasks: chain.tasks.apply(&write),
                decision: chain.decision.write(write),
                ids: chain.ids,
            }
        }
        _ => chain,
    }
}

/// `AttemptStartOut`.
#[must_use]
pub fn start_out(attempt_id: Id, number: i64, task: &Task, manifest: &ExecutionManifest) -> Value {
    let repositories: Vec<Value> = manifest
        .repositories
        .iter()
        .map(|entry| {
            json!({
                "repository": entry.repository,
                "checkout_path": entry.checkout_path,
                "branch": entry.branch,
                "base_sha": entry.sha,
                "base": entry.base,
            })
        })
        .collect();
    json!({
        "attempt_id": attempt_id.to_string(),
        "attempt_number": number,
        "task_key": task.key.to_string(),
        "repositories": repositories,
        "agent_checks": manifest.agent_checks,
    })
}

/// The text a task reason is cut to (`reason[:200]`).
#[must_use]
pub fn reason_text(text: &str) -> String {
    text.chars().take(REASON_LIMIT).collect()
}
