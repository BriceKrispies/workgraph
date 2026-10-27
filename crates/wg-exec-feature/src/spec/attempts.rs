//! The attempt rows of the run flow as decisions: a new attempt, a status change and the finish of
//! an attempt (`new_attempt`, `set_attempt_status` and `finish_attempt` of
//! `core/services/attempts.py`, as the run orchestration calls them), plus the task snapshot an
//! attempt is created from.
//!
//! Each function takes the row it changes and the stamp, and returns the new row with the event
//! that records the change; the caller chains them into one [`wg_model::Decision`].

use serde_json::{Map, Value, json};
use wg_exec::attempt::{
    FinishPlan, check_attempt_transition, duration_ms, finish_plan, sanitize_usage,
};
use wg_model::entity::{AgentAttempt, Real, Task};
use wg_model::event::{
    AttemptCreated, AttemptFinished, AttemptStatusChanged, EventDraft, EventPayload,
};
use wg_model::stamp::{IdSeq, Stamp};
use wg_types::{AttemptStatus, Id, WgError};

/// The longest error an attempt keeps (`error[:20_000]`).
pub const ERROR_LIMIT: usize = 20_000;

/// The longest error an attempt event shows (`(error or "")[:1000]`).
pub const EVENT_ERROR_LIMIT: usize = 1000;

/// The `input_task_snapshot` of an attempt (`task_snapshot`).
pub fn task_snapshot(task: &Task) -> Map<String, Value> {
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

/// What a new attempt is made of (the keyword arguments of `new_attempt`).
#[derive(Debug, Clone)]
pub struct NewAttempt {
    pub number: i64,
    pub status: AttemptStatus,
    pub provider: String,
    pub adapter: String,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub parent_attempt_id: Option<Id>,
    pub retry_of_attempt_id: Option<Id>,
    pub rerun_of_attempt_id: Option<Id>,
    pub config_snapshot: Map<String, Value>,
    pub telemetry: Map<String, Value>,
}

/// `new_attempt`: the attempt row of `task` and its `attempt.created` event. The id of the
/// attempt and of the event are the next two of `ids`.
pub fn new_attempt(
    stamp: &Stamp,
    ids: IdSeq,
    task: &Task,
    spec: NewAttempt,
) -> (AgentAttempt, EventDraft, IdSeq) {
    let ([attempt_id, event_id], ids) = ids.take::<2>();
    let now = stamp.at;
    let running = spec.status == AttemptStatus::Running;
    let attempt = AgentAttempt {
        id: attempt_id,
        workspace_id: task.workspace_id,
        task_id: task.id,
        number: spec.number,
        status: spec.status,
        provider: spec.provider.clone(),
        adapter: spec.adapter.clone(),
        agent_id: spec.agent_id,
        model: spec.model.clone(),
        session_id: None,
        parent_attempt_id: spec.parent_attempt_id,
        retry_of_attempt_id: spec.retry_of_attempt_id,
        rerun_of_attempt_id: spec.rerun_of_attempt_id,
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
        telemetry: spec.telemetry,
        modified_repository_ids: rpds::VectorSync::new_sync(),
        retries: 0,
        cancel_requested: false,
        updated_at: now,
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(task.workspace_id),
        attempt.id,
        EventPayload::AttemptCreated(AttemptCreated {
            task_id: task.id,
            task_key: task.key.clone(),
            number: spec.number,
            provider: spec.provider,
            adapter: spec.adapter,
            model: spec.model,
            status: spec.status,
            parent_attempt_id: spec.parent_attempt_id,
            retry_of_attempt_id: spec.retry_of_attempt_id,
            rerun_of_attempt_id: spec.rerun_of_attempt_id,
        }),
    )
    .with_attempt(Some(attempt.id));
    (attempt, event, ids)
}

/// Whole milliseconds from `from` to `to`, as `int(delta.total_seconds() * 1000)` gives them.
pub fn elapsed_ms(from: wg_types::Timestamp, to: wg_types::Timestamp) -> i64 {
    let micros = (to.to_offset_datetime() - from.to_offset_datetime()).whole_microseconds();
    let seconds = micros as f64 / 1_000_000.0;
    (seconds * 1000.0).trunc() as i64
}

/// `set_attempt_status`: the attempt in `status`, with the `attempt.status_changed` event. The
/// same status changes nothing. `reason` and similar keys ride along in the event's open data.
pub fn set_status(
    stamp: &Stamp,
    event_id: Id,
    attempt: &AgentAttempt,
    status: AttemptStatus,
    extra: Map<String, Value>,
) -> Result<Option<(AgentAttempt, EventDraft)>, WgError> {
    if attempt.status == status {
        return Ok(None);
    }
    check_attempt_transition(attempt.status, status)?;
    let now = stamp.at;
    let starts = status == AttemptStatus::Running && attempt.started_at.is_none();
    let changed = AgentAttempt {
        status,
        started_at: if starts {
            Some(now)
        } else {
            attempt.started_at
        },
        queue_wait_ms: if starts {
            attempt
                .queued_at
                .map(|queued| elapsed_ms(queued, now).max(0))
                .or(attempt.queue_wait_ms)
        } else {
            attempt.queue_wait_ms
        },
        updated_at: now,
        ..attempt.clone()
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(attempt.workspace_id),
        attempt.id,
        EventPayload::AttemptStatusChanged(AttemptStatusChanged {
            from: attempt.status,
            to: status,
            task_id: attempt.task_id,
            extra,
        }),
    )
    .with_attempt(Some(attempt.id));
    Ok(Some((changed, event)))
}

/// What `finish_attempt` records besides the status (its keyword arguments).
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

/// `finish_attempt`: the attempt finished as `status` and the event that records it. `None` when it
/// already finished with this status.
pub fn finish(
    stamp: &Stamp,
    event_id: Id,
    attempt: &AgentAttempt,
    status: AttemptStatus,
    finish: &Finish,
) -> Result<Option<(AgentAttempt, EventDraft)>, WgError> {
    let FinishPlan::Apply { event } = finish_plan(attempt.status, status).map_err(|error| {
        // The message names the attempt, as `finish_attempt` does.
        if wg_exec::attempt::is_terminal(attempt.status) && attempt.status != status {
            WgError::validation(format!(
                "attempt {} already finished as {}",
                attempt.id, attempt.status
            ))
        } else {
            error
        }
    })?
    else {
        return Ok(None);
    };
    let now = stamp.at;
    let clean = finish
        .usage
        .as_ref()
        .map(sanitize_usage)
        .unwrap_or_default();
    let elapsed = attempt.started_at.map(|started| elapsed_ms(started, now));
    let duration = duration_ms(finish.duration_ms, elapsed).or(attempt.duration_ms);
    let error_text = finish.error.as_deref().filter(|text| !text.is_empty());
    let finished = AgentAttempt {
        status,
        finished_at: Some(now),
        exit_reason: finish.exit_reason.clone(),
        exit_code: finish.exit_code,
        error: error_text
            .map(|text| text.chars().take(ERROR_LIMIT).collect())
            .or_else(|| attempt.error.clone()),
        summary: finish
            .summary
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|text| text.chars().take(wg_exec::run::SUMMARY_LIMIT).collect())
            .or_else(|| attempt.summary.clone()),
        usage: if clean.is_empty() {
            attempt.usage.clone()
        } else {
            clean.clone()
        },
        usage_source: if clean.is_empty() {
            attempt.usage_source.clone()
        } else {
            Some(
                finish
                    .usage_source
                    .clone()
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "measured".to_owned()),
            )
        },
        cost_usd: finish.cost_usd.map(Real).or(attempt.cost_usd),
        cost_source: if finish.cost_usd.is_some() {
            Some(
                finish
                    .cost_source
                    .clone()
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "measured".to_owned()),
            )
        } else {
            attempt.cost_source.clone()
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
        duration_ms: duration,
        updated_at: now,
        ..attempt.clone()
    };
    let body = AttemptFinished {
        task_id: attempt.task_id,
        from: attempt.status,
        status,
        exit_reason: finish.exit_reason.clone(),
        exit_code: finish.exit_code,
        duration_ms: finished.duration_ms,
        error: finish
            .error
            .as_deref()
            .map(|text| text.chars().take(EVENT_ERROR_LIMIT).collect::<String>())
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
    Ok(Some((finished, draft)))
}
