//! The pure half of the attempt writes (`core/services/attempts.py` and `orchestration/runs.py`):
//! a new attempt row and its `attempt.created` event, a status change, a finish, the task moves
//! that follow an attempt and the escalation of a task whose attempts keep failing.
//!
//! Every function takes the chain of changes made so far (an [`Applied`]: the decision, the ids
//! still free and the task slice after the writes) and returns the chain with its own changes
//! added, so a caller composes the steps of one Python session into one decision. The shell
//! (`service::attempt_runs`) reads the rows, asks here what to write and applies the answer.

use serde_json::{Map, Value, json};
use wg_exec::attempt::{
    FinishPlan, check_attempt_transition, counts_as_failure, duration_ms, escalates, finish_plan,
    sanitize_usage,
};
use wg_exec::manifest::ExecutionManifest;
use wg_model::Decision;
use wg_model::TaskSlice;
use wg_model::entity::{AgentAttempt, Real, TaskTag};
use wg_model::event::{
    AttemptCreated, AttemptFinished, AttemptStatusChanged, EventDraft, EventPayload, TaskEscalated,
};
use wg_model::slice::StackSlice;
use wg_model::stamp::Stamp;
use wg_model::{Task, Write};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::transition::{Applied, Change, transition};
use wg_tasks_feature::spec::bodies::ClaimRead;
use wg_tasks_feature::spec::claims::claim_task;
use wg_types::{AttemptStatus, ErrorCode, Id, TaskStatus, Timestamp, WgError};

/// The longest `error` an attempt keeps (`error[:20_000]`).
const ERROR_LIMIT: usize = 20_000;
/// The longest `summary` an attempt keeps (`summary[:50_000]`).
const SUMMARY_LIMIT: usize = 50_000;
/// The longest `error` the finishing event repeats (`error[:1000]`).
const EVENT_ERROR_LIMIT: usize = 1_000;
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

/// The `input_task_snapshot` of an attempt (`task_snapshot`).
fn task_snapshot(task: &Task) -> Map<String, Value> {
    [
        ("key", json!(task.key.to_string())),
        ("version", json!(task.version)),
        ("title", json!(task.title)),
        ("description", json!(task.description)),
        (
            "acceptance_criteria",
            Value::Array(task.acceptance_criteria.iter().map(|c| json!(c)).collect()),
        ),
        ("priority", json!(task.priority.as_str())),
        ("category", json!(task.category)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect()
}

/// The first `limit` characters of `text`.
fn clip(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// Milliseconds from `from` to `to` as `int(delta.total_seconds() * 1000)` gives them.
fn elapsed_ms(from: Timestamp, to: Timestamp) -> i64 {
    let micros = (to.to_offset_datetime() - from.to_offset_datetime()).whole_microseconds();
    ((micros as f64 / 1_000_000.0) * 1000.0).trunc() as i64
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
    let now = stamp.at;
    let running = spec.status == AttemptStatus::Running;
    let (attempt_id, ids) = start.ids.next();
    let (event_id, ids) = ids.next();
    let attempt = AgentAttempt {
        id: attempt_id,
        workspace_id: task.workspace_id,
        task_id: task.id,
        number,
        status: spec.status,
        provider: spec.provider.clone(),
        adapter: spec.adapter.clone(),
        agent_id: spec.agent_id,
        model: spec.model.clone(),
        session_id: spec.session_id,
        parent_attempt_id: spec.parent_attempt_id,
        retry_of_attempt_id: None,
        rerun_of_attempt_id: None,
        created_by: stamp.actor.to_string(),
        created_at: now,
        queued_at: Some(now),
        started_at: running.then_some(now),
        finished_at: None,
        queue_wait_ms: running.then_some(0),
        duration_ms: None,
        exit_reason: None,
        exit_code: None,
        error: None,
        summary: None,
        input_task_version: Some(task.version),
        input_task_snapshot: task_snapshot(task),
        prompt_artifact_id: None,
        config_snapshot: spec.config_snapshot,
        environment: Map::new(),
        baseline_manifest_id: None,
        outcome_manifest_id: None,
        usage: Map::new(),
        usage_source: None,
        cost_usd: None,
        cost_source: None,
        telemetry: Map::new(),
        modified_repository_ids: rpds::VectorSync::new_sync(),
        retries: 0,
        cancel_requested: false,
        updated_at: now,
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(task.workspace_id),
        attempt_id,
        EventPayload::AttemptCreated(AttemptCreated {
            task_id: task.id,
            task_key: task.key.clone(),
            number,
            provider: spec.provider,
            adapter: spec.adapter,
            model: spec.model,
            status: spec.status,
            parent_attempt_id: spec.parent_attempt_id,
            retry_of_attempt_id: None,
            rerun_of_attempt_id: None,
        }),
    )
    .with_attempt(Some(attempt_id));
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
    if attempt.status == status {
        return Ok((start, attempt.clone()));
    }
    check_attempt_transition(attempt.status, status)?;
    let now = stamp.at;
    let first_run = status == AttemptStatus::Running && attempt.started_at.is_none();
    let moved = AgentAttempt {
        status,
        started_at: if first_run {
            Some(now)
        } else {
            attempt.started_at
        },
        queue_wait_ms: match (first_run, attempt.queued_at) {
            (true, Some(queued)) => Some(elapsed_ms(queued, now).max(0)),
            _ => attempt.queue_wait_ms,
        },
        updated_at: now,
        ..attempt.clone()
    };
    let (event_id, ids) = start.ids.next();
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(attempt.workspace_id),
        attempt.id,
        EventPayload::AttemptStatusChanged(AttemptStatusChanged {
            from: attempt.status,
            to: status,
            task_id: attempt.task_id,
            extra: Map::new(),
        }),
    )
    .with_attempt(Some(attempt.id));
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

/// What `finish_attempt` records besides the status.
#[derive(Debug, Clone, Default)]
pub struct Finish {
    pub exit_reason: Option<String>,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    pub summary: Option<String>,
    pub usage: Option<Map<String, Value>>,
    pub usage_source: Option<String>,
    pub cost_usd: Option<f64>,
    pub cost_source: Option<String>,
    pub telemetry: Option<Map<String, Value>>,
    pub duration_ms: Option<i64>,
}

/// The error of finishing an attempt that finished differently already.
fn refused(attempt: &AgentAttempt, error: WgError) -> WgError {
    if wg_exec::attempt::is_terminal(attempt.status) {
        WgError::new(
            ErrorCode::ValidationFailed,
            format!(
                "attempt {} already finished as {}",
                attempt.id, attempt.status
            ),
        )
    } else {
        error
    }
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
    let FinishPlan::Apply { event } =
        finish_plan(attempt.status, status).map_err(|error| refused(attempt, error))?
    else {
        return Ok((start, attempt.clone()));
    };
    let now = stamp.at;
    let usage = finish
        .usage
        .as_ref()
        .map(sanitize_usage)
        .filter(|clean| !clean.is_empty());
    let elapsed = attempt
        .started_at
        .map(|started| elapsed_ms(started, now).max(0));
    let error = finish
        .error
        .as_deref()
        .filter(|text| !text.is_empty())
        .map(|text| clip(text, ERROR_LIMIT));
    let row = AgentAttempt {
        status,
        finished_at: Some(now),
        exit_reason: finish.exit_reason.clone(),
        exit_code: finish.exit_code,
        error: error.or_else(|| attempt.error.clone()),
        summary: finish
            .summary
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|text| clip(text, SUMMARY_LIMIT))
            .or_else(|| attempt.summary.clone()),
        usage: usage.clone().unwrap_or_else(|| attempt.usage.clone()),
        usage_source: match &usage {
            Some(_) => Some(
                finish
                    .usage_source
                    .clone()
                    .unwrap_or_else(|| "measured".to_owned()),
            ),
            None => attempt.usage_source.clone(),
        },
        cost_usd: finish.cost_usd.map(Real).or(attempt.cost_usd),
        cost_source: match finish.cost_usd {
            Some(_) => Some(
                finish
                    .cost_source
                    .clone()
                    .unwrap_or_else(|| "measured".to_owned()),
            ),
            None => attempt.cost_source.clone(),
        },
        telemetry: match finish.telemetry.as_ref().filter(|extra| !extra.is_empty()) {
            Some(extra) => attempt
                .telemetry
                .iter()
                .chain(extra.iter())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            None => attempt.telemetry.clone(),
        },
        duration_ms: duration_ms(finish.duration_ms, elapsed).or(attempt.duration_ms),
        updated_at: now,
        ..attempt.clone()
    };
    let (event_id, ids) = start.ids.next();
    let body = AttemptFinished {
        task_id: attempt.task_id,
        from: attempt.status,
        status,
        exit_reason: finish.exit_reason,
        exit_code: finish.exit_code,
        duration_ms: row.duration_ms,
        error: finish
            .error
            .as_deref()
            .map(|text| clip(text, EVENT_ERROR_LIMIT))
            .filter(|text| !text.is_empty()),
    };
    let payload = match event {
        "attempt.succeeded" => EventPayload::AttemptSucceeded(body),
        "attempt.cancelled" => EventPayload::AttemptCancelled(body),
        _ => EventPayload::AttemptFailed(body),
    };
    let draft = EventDraft::new(
        stamp,
        event_id,
        Some(attempt.workspace_id),
        attempt.id,
        payload,
    )
    .with_attempt(Some(attempt.id));
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

/// A step that needs work another task owns (running checks, stacks).
#[must_use]
pub fn not_ported(what: &str) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("{what} is not ported yet"),
    )
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
    clip(text, REASON_LIMIT)
}
