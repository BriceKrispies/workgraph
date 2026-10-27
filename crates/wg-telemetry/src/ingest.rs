//! The rows and events hook ingestion writes (the pure steps of `process_record`, `_apply`,
//! `_apply_transcript`, `_child_attempt` and `_roll_up` in `integrations/claude_code/ingest.py`, and of
//! `new_attempt` and `finish_attempt` in `core/services/attempts.py`).
//!
//! The shell reads the rows each step needs, asks a function here for the row or event to write, and
//! applies it. Every function takes the clock reading (`now`, the stamp's) as an argument.

use serde_json::{Map, Value, json};
use wg_exec::attempt::{FinishPlan, finish_plan, is_terminal, sanitize_usage};
use wg_model::entity::{
    AgentAttempt, AgentInternalTask, AgentSession, HookEvent, SubagentRun, Task, ToolEvent,
};
use wg_model::event::{
    AgentInternalTaskChanged, AgentSessionBound, AgentSessionEnded, AgentSessionStarted,
    AgentSubagentChanged, AttemptCreated, AttemptFinished, AttemptSubagentBound,
};
use wg_model::{EventDraft, EventPayload, Stamp};
use wg_types::{AttemptStatus, Id, Timestamp, WgError, parse_id};

use crate::binding::Resolved;
use crate::hook::{
    HookFields, Normalised, ToolEventRow, internal_task_description, internal_task_subject,
    last_message_excerpt, parse_utc, tool_counters,
};
use crate::json::truthy;
use crate::transcript::TranscriptStats;

/// The provider every hook row records.
pub const PROVIDER: &str = "claude_code";
/// The `usage_source` of usage measured from a subagent transcript.
pub const TRANSCRIPT_USAGE_SOURCE: &str = "claude_code_transcript";
/// The adapter of an attempt a subagent transcript creates.
pub const SUBAGENT_ADAPTER: &str = "claude_code_subagent";
/// The exit reason of a subagent attempt that stopped.
pub const SUBAGENT_STOPPED: &str = "subagent_stopped";
/// The binding source of a session nothing bound.
pub const UNBOUND: &str = "unbound";
/// The status a new internal task has.
pub const INTERNAL_CREATED: &str = "created";
/// The status a completed internal task has.
pub const INTERNAL_COMPLETED: &str = "completed";

/// The launch context a hook record carries (`record["env"]`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookEnv {
    pub attempt_id: Option<String>,
    pub task_id: Option<String>,
    pub workspace: Option<String>,
}

/// The launch context of a record: its `env` object when it has one, with only text values kept.
pub fn hook_env(record: &Value) -> HookEnv {
    let env = record.get("env").filter(|env| truthy(Some(env)));
    let text = |key: &str| {
        env.and_then(|env| env.get(key))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    HookEnv {
        attempt_id: text("attempt_id"),
        task_id: text("task_id"),
        workspace: text("workspace"),
    }
}

/// The id a text names, when it is in canonical form (Python compares id text exactly).
pub fn canonical_id(text: &str) -> Option<Id> {
    parse_id(text).ok().filter(|id| id.to_string() == text)
}

/// A string field the payload carries, `None` when absent or empty.
pub fn present(text: Option<&String>) -> Option<&str> {
    text.map(String::as_str).filter(|text| !text.is_empty())
}

/// The session a first hook of an external session creates.
pub fn new_session(
    id: Id,
    external_session_id: &str,
    resolved: &Resolved,
    fields: &HookFields,
    at: Timestamp,
    now: Timestamp,
) -> AgentSession {
    AgentSession {
        id,
        provider: PROVIDER.to_owned(),
        external_session_id: external_session_id.to_owned(),
        workspace_id: resolved.workspace_id.as_deref().and_then(canonical_id),
        attempt_id: resolved.attempt_id.as_deref().and_then(canonical_id),
        task_id: resolved.task_id.as_deref().and_then(canonical_id),
        binding_source: resolved.source.as_str().to_owned(),
        cwd: fields.cwd.clone(),
        model: None,
        transcript_path: fields.transcript_path.clone(),
        started_at: Some(at),
        ended_at: None,
        end_reason: None,
        tool_calls: 0,
        tool_failures: 0,
        created_at: now,
        updated_at: now,
    }
}

/// An unbound session after a later hook found a binding for it; `None` when it is still unbound.
pub fn rebound(
    session: &AgentSession,
    resolved: &Resolved,
    now: Timestamp,
) -> Option<AgentSession> {
    (resolved.source.as_str() != UNBOUND).then(|| AgentSession {
        workspace_id: resolved.workspace_id.as_deref().and_then(canonical_id),
        attempt_id: resolved.attempt_id.as_deref().and_then(canonical_id),
        task_id: resolved.task_id.as_deref().and_then(canonical_id),
        binding_source: resolved.source.as_str().to_owned(),
        updated_at: now,
        ..session.clone()
    })
}

/// The ledger row of one received hook.
pub fn hook_row(
    id: Id,
    normalised: &Normalised,
    session: Option<&AgentSession>,
    at: Timestamp,
    source: &str,
) -> HookEvent {
    HookEvent {
        id,
        dedupe_key: normalised.dedupe_key.clone(),
        provider: PROVIDER.to_owned(),
        hook_event_name: normalised
            .event
            .chars()
            .take(crate::hook::EVENT_NAME_CHARS)
            .collect(),
        external_session_id: normalised.fields.session_id.clone(),
        session_id: session.map(|session| session.id),
        attempt_id: session.and_then(|session| session.attempt_id),
        payload: Some(normalised.sanitized.clone()),
        payload_purged_at: None,
        received_at: at,
        source: source.to_owned(),
    }
}

/// The tool event row of a tool phase hook.
pub fn tool_row(
    id: Id,
    hook_id: Id,
    session: Option<&AgentSession>,
    fields: &HookFields,
    row: ToolEventRow,
    at: Timestamp,
) -> ToolEvent {
    ToolEvent {
        id,
        hook_event_id: hook_id,
        session_id: session.map(|session| session.id),
        attempt_id: session.and_then(|session| session.attempt_id),
        external_agent_id: fields.agent_id.clone(),
        phase: row.phase.to_owned(),
        tool_name: row.tool_name,
        tool_use_id: row.tool_use_id,
        duration_ms: row.duration_ms,
        error_excerpt: row.error_excerpt,
        summary: row.summary,
        occurred_at: at,
    }
}

/// Whether a payload carries a `duration_ms` that does not fit the database integer. Python raises
/// `OverflowError` binding it, which fails the whole ingestion.
pub fn duration_overflows(validated: &Map<String, Value>, fields: &HookFields) -> bool {
    fields.duration_ms.is_none()
        && validated
            .get("duration_ms")
            .is_some_and(|value| value.is_number())
}

/// The failure of a `duration_ms` beyond the database integer.
pub fn overflow_failure() -> (&'static str, String) {
    (
        "OverflowError",
        "Python int too large to convert to SQLite INTEGER".to_owned(),
    )
}

/// The session after a completed tool phase: its counters moved.
pub fn counted(session: &AgentSession, phase: &str, now: Timestamp) -> AgentSession {
    let (calls, failures) = tool_counters(phase);
    AgentSession {
        tool_calls: session.tool_calls + calls,
        tool_failures: session.tool_failures + failures,
        updated_at: now,
        ..session.clone()
    }
}

/// The session after `SessionStart`: when it started, where, and the model the hook names.
pub fn started(
    session: &AgentSession,
    fields: &HookFields,
    at: Timestamp,
    now: Timestamp,
) -> AgentSession {
    AgentSession {
        started_at: session.started_at.or(Some(at)),
        cwd: fields
            .cwd
            .clone()
            .filter(|cwd| !cwd.is_empty())
            .or_else(|| session.cwd.clone()),
        model: fields
            .model
            .as_str()
            .map(str::to_owned)
            .or_else(|| session.model.clone()),
        updated_at: now,
        ..session.clone()
    }
}

/// The `agent_session.started` event of a session.
pub fn started_event(session: &AgentSession, fields: &HookFields) -> EventPayload {
    EventPayload::AgentSessionStarted(AgentSessionStarted {
        binding: json!(session.binding_source),
        external_session_id: session.external_session_id.clone(),
        provider: PROVIDER.to_owned(),
        source: fields.source.clone(),
        task_id: session.task_id,
    })
}

/// The session after `SessionEnd`.
pub fn ended(
    session: &AgentSession,
    fields: &HookFields,
    at: Timestamp,
    now: Timestamp,
) -> AgentSession {
    AgentSession {
        ended_at: Some(at),
        end_reason: fields.reason.clone(),
        updated_at: now,
        ..session.clone()
    }
}

/// The `agent_session.ended` event of a session.
pub fn ended_event(session: &AgentSession) -> EventPayload {
    EventPayload::AgentSessionEnded(AgentSessionEnded {
        reason: session.end_reason.clone(),
        tool_calls: session.tool_calls,
        tool_failures: session.tool_failures,
    })
}

/// The run a first `SubagentStart` or `SubagentStop` of an agent creates.
pub fn new_run(
    id: Id,
    session: &AgentSession,
    agent_id: &str,
    fields: &HookFields,
    at: Timestamp,
) -> SubagentRun {
    SubagentRun {
        id,
        session_id: session.id,
        external_agent_id: agent_id.to_owned(),
        agent_type: fields.agent_type.clone(),
        attempt_id: None,
        started_at: (fields.hook_event_name == "SubagentStart").then_some(at),
        stopped_at: None,
        last_message_excerpt: None,
        transcript_path: None,
        requested_model: None,
        model: None,
        usage: None,
        telemetry: None,
        task_id: None,
    }
}

/// The run after `SubagentStop`.
pub fn stopped(run: &SubagentRun, fields: &HookFields, at: Timestamp) -> SubagentRun {
    SubagentRun {
        stopped_at: Some(at),
        transcript_path: fields.agent_transcript_path.clone(),
        last_message_excerpt: last_message_excerpt(fields.last_assistant_message.as_deref()),
        ..run.clone()
    }
}

/// The `agent_session.subagent_started` or `subagent_stopped` event.
pub fn subagent_event(run: &SubagentRun, fields: &HookFields) -> EventPayload {
    let changed = AgentSubagentChanged {
        agent_id: fields.agent_id.clone().unwrap_or_default(),
        agent_type: fields.agent_type.clone(),
        child_attempt_id: run.attempt_id,
    };
    if fields.hook_event_name == "SubagentStart" {
        EventPayload::AgentSessionSubagentStarted(changed)
    } else {
        EventPayload::AgentSessionSubagentStopped(changed)
    }
}

/// The internal task a first `TaskCreated` or `TaskCompleted` of an agent task creates.
pub fn new_internal_task(
    id: Id,
    session: &AgentSession,
    task_id: &str,
    fields: &HookFields,
    now: Timestamp,
) -> AgentInternalTask {
    AgentInternalTask {
        id,
        session_id: session.id,
        external_task_id: task_id.to_owned(),
        subject: internal_task_subject(fields.task_subject.as_deref()),
        description: internal_task_description(fields.task_description.as_deref()),
        status: INTERNAL_CREATED.to_owned(),
        task_id: None,
        external_agent_id: fields.agent_id.clone(),
        created_at: now,
        completed_at: None,
    }
}

/// The internal task after the hook: linked to the Workgraph task it names, and completed on
/// `TaskCompleted`.
pub fn internal_task_updated(
    task: &AgentInternalTask,
    mapped: Option<Id>,
    fields: &HookFields,
    at: Timestamp,
) -> AgentInternalTask {
    let completed = fields.hook_event_name == "TaskCompleted";
    AgentInternalTask {
        task_id: mapped.or(task.task_id),
        status: if completed {
            INTERNAL_COMPLETED.to_owned()
        } else {
            task.status.clone()
        },
        completed_at: if completed {
            Some(at)
        } else {
            task.completed_at
        },
        ..task.clone()
    }
}

/// The `agent_session.internal_task_created` or `internal_task_completed` event.
pub fn internal_task_event(task: &AgentInternalTask, fields: &HookFields) -> EventPayload {
    let changed = AgentInternalTaskChanged {
        authoritative: false,
        external_task_id: task.external_task_id.clone(),
        mapped_task_id: task.task_id,
        subject: task.subject.clone(),
    };
    if fields.hook_event_name == "TaskCompleted" {
        EventPayload::AgentSessionInternalTaskCompleted(changed)
    } else {
        EventPayload::AgentSessionInternalTaskCreated(changed)
    }
}

/// The attempt after a session rolled up its hook counters.
pub fn rolled_up(
    attempt: &AgentAttempt,
    session: &AgentSession,
    subagents: usize,
    now: Timestamp,
) -> AgentAttempt {
    AgentAttempt {
        telemetry: crate::hook::rollup_telemetry(
            &attempt.telemetry,
            session.tool_calls,
            session.tool_failures,
            subagents,
        ),
        updated_at: now,
        ..attempt.clone()
    }
}

/// What an attempt a hook creates is made of.
#[derive(Clone, Debug)]
pub struct AttemptSpec {
    pub adapter: &'static str,
    pub agent_id: String,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub parent_attempt_id: Option<Id>,
    /// Replaces the start time (`started_at = ...` after `new_attempt`).
    pub started_at: Option<Timestamp>,
    pub telemetry: Map<String, Value>,
}

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

/// A running attempt of `task` for a hook (`new_attempt` with status running) and its
/// `attempt.created` event. `number` is the task's next attempt number.
pub fn new_attempt(
    stamp: &Stamp,
    (attempt_id, event_id): (Id, Id),
    task: &Task,
    number: i64,
    spec: AttemptSpec,
) -> (AgentAttempt, EventDraft) {
    let now = stamp.at;
    let attempt = AgentAttempt {
        id: attempt_id,
        workspace_id: task.workspace_id,
        task_id: task.id,
        number,
        status: AttemptStatus::Running,
        provider: PROVIDER.to_owned(),
        adapter: spec.adapter.to_owned(),
        agent_id: Some(spec.agent_id),
        model: spec.model,
        session_id: spec.session_id,
        parent_attempt_id: spec.parent_attempt_id,
        retry_of_attempt_id: None,
        rerun_of_attempt_id: None,
        created_by: stamp.actor.to_string(),
        created_at: now,
        queued_at: Some(now),
        started_at: Some(spec.started_at.unwrap_or(now)),
        finished_at: None,
        queue_wait_ms: Some(0),
        duration_ms: None,
        exit_reason: None,
        exit_code: None,
        error: None,
        summary: None,
        input_task_version: Some(task.version),
        input_task_snapshot: task_snapshot(task),
        prompt_artifact_id: None,
        config_snapshot: Map::new(),
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
            number,
            provider: PROVIDER.to_owned(),
            adapter: attempt.adapter.clone(),
            model: attempt.model.clone(),
            status: AttemptStatus::Running,
            parent_attempt_id: attempt.parent_attempt_id,
            retry_of_attempt_id: None,
            rerun_of_attempt_id: None,
        }),
    )
    .with_attempt(Some(attempt.id));
    (attempt, event)
}

/// What a hook records when it finishes an attempt.
#[derive(Clone, Debug, Default)]
pub struct Finish {
    pub exit_reason: Option<String>,
    pub summary: Option<String>,
    pub usage: Option<Map<String, Value>>,
    pub usage_source: Option<String>,
    pub telemetry: Option<Map<String, Value>>,
}

/// Microseconds from `from` to `to`.
fn micros_between(from: Timestamp, to: Timestamp) -> i128 {
    (to.to_offset_datetime() - from.to_offset_datetime()).whole_microseconds()
}

/// The elapsed whole milliseconds from `from` to `to`, as `int(delta.total_seconds() * 1000)` gives them.
pub fn elapsed_ms(from: Timestamp, to: Timestamp) -> i64 {
    let seconds = micros_between(from, to) as f64 / 1_000_000.0;
    (seconds * 1000.0).trunc() as i64
}

/// An attempt finished as `status` (`finish_attempt`) and the event that records it. `None` when the attempt
/// already finished with this status.
pub fn finish_attempt(
    stamp: &Stamp,
    event_id: Id,
    attempt: &AgentAttempt,
    status: AttemptStatus,
    finish: Finish,
) -> Result<Option<(AgentAttempt, EventDraft)>, WgError> {
    let plan = finish_plan(attempt.status, status)
        .map_err(|error| already_finished(attempt, status, error))?;
    let FinishPlan::Apply { event } = plan else {
        return Ok(None);
    };
    let now = stamp.at;
    let clean = finish
        .usage
        .as_ref()
        .map(sanitize_usage)
        .unwrap_or_default();
    let duration = attempt
        .started_at
        .map(|started| elapsed_ms(started, now).max(0));
    let finished = AgentAttempt {
        status,
        finished_at: Some(now),
        exit_reason: finish.exit_reason.clone(),
        exit_code: None,
        summary: finish
            .summary
            .clone()
            .filter(|text| !text.is_empty())
            .map(|text| text.chars().take(50_000).collect())
            .or_else(|| attempt.summary.clone()),
        usage: if clean.is_empty() {
            attempt.usage.clone()
        } else {
            clean.clone()
        },
        usage_source: if clean.is_empty() {
            attempt.usage_source.clone()
        } else {
            Some(finish.usage_source.unwrap_or_else(|| "measured".to_owned()))
        },
        telemetry: match finish.telemetry.filter(|extra| !extra.is_empty()) {
            Some(extra) => attempt
                .telemetry
                .iter()
                .chain(extra.iter())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            None => attempt.telemetry.clone(),
        },
        duration_ms: duration.or(attempt.duration_ms),
        updated_at: now,
        ..attempt.clone()
    };
    let payload = AttemptFinished {
        task_id: attempt.task_id,
        from: attempt.status,
        status,
        exit_reason: finish.exit_reason,
        exit_code: None,
        duration_ms: finished.duration_ms,
        error: None,
    };
    let payload = match event {
        "attempt.succeeded" => EventPayload::AttemptSucceeded(payload),
        "attempt.cancelled" => EventPayload::AttemptCancelled(payload),
        _ => EventPayload::AttemptFailed(payload),
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

/// The error of finishing an attempt that is already finished differently, with the attempt named as Python
/// names it.
fn already_finished(attempt: &AgentAttempt, status: AttemptStatus, error: WgError) -> WgError {
    if is_terminal(attempt.status) && attempt.status != status {
        WgError::validation(format!(
            "attempt {} already finished as {}",
            attempt.id, attempt.status
        ))
    } else {
        error
    }
}

/// What a subagent transcript changes on the run, its child attempt and the attempt it binds to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TranscriptApplied {
    pub parsed: bool,
    pub linked: bool,
    pub attempt_created: bool,
    pub attempt_bound: bool,
}

/// The telemetry object stored for a transcript: every field but `usage`.
pub fn transcript_telemetry(stats: &TranscriptStats) -> Map<String, Value> {
    match stats.to_json() {
        Value::Object(map) => map.into_iter().filter(|(key, _)| key != "usage").collect(),
        _ => Map::new(),
    }
}

/// The usage a transcript reports, or `None` when it has no turns.
pub fn transcript_usage(stats: &TranscriptStats) -> Option<Map<String, Value>> {
    (stats.turns > 0).then(|| match stats.to_json() {
        Value::Object(map) => map
            .get("usage")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        _ => Map::new(),
    })
}

/// The run after its transcript was read: the measured model, usage and telemetry, linked to `task`.
pub fn run_with_transcript(
    run: &SubagentRun,
    stats: &TranscriptStats,
    task: Option<&Task>,
) -> SubagentRun {
    SubagentRun {
        requested_model: stats.requested_model.clone(),
        model: stats.model.clone(),
        telemetry: Some(transcript_telemetry(stats)),
        usage: transcript_usage(stats).or_else(|| run.usage.clone()),
        task_id: task.map(|task| task.id).or(run.task_id),
        ..run.clone()
    }
}

/// A child attempt after its subagent's transcript: the measured model and usage.
pub fn child_with_transcript(
    child: &AgentAttempt,
    stats: &TranscriptStats,
    now: Timestamp,
) -> AgentAttempt {
    let usage = transcript_usage(stats);
    AgentAttempt {
        model: stats.model.clone(),
        usage: usage.clone().unwrap_or_else(|| child.usage.clone()),
        usage_source: usage
            .map(|_| TRANSCRIPT_USAGE_SOURCE.to_owned())
            .or_else(|| child.usage_source.clone()),
        updated_at: now,
        ..child.clone()
    }
}

/// An orchestrator's attempt after a subagent run bound to it: the measured facts, not the status.
pub fn bound_attempt(
    attempt: &AgentAttempt,
    stats: &TranscriptStats,
    now: Timestamp,
) -> AgentAttempt {
    let usage = transcript_usage(stats);
    AgentAttempt {
        model: stats.model.clone().or_else(|| attempt.model.clone()),
        usage: usage.clone().unwrap_or_else(|| attempt.usage.clone()),
        usage_source: usage
            .map(|_| TRANSCRIPT_USAGE_SOURCE.to_owned())
            .or_else(|| attempt.usage_source.clone()),
        telemetry: attempt
            .telemetry
            .iter()
            .chain(transcript_telemetry(stats).iter())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        updated_at: now,
        ..attempt.clone()
    }
}

/// The `attempt.subagent_bound` event.
pub fn subagent_bound_event(
    stamp: &Stamp,
    event_id: Id,
    attempt: &AgentAttempt,
    run: &SubagentRun,
    task: &Task,
    stats: &TranscriptStats,
) -> EventDraft {
    EventDraft::new(
        stamp,
        event_id,
        Some(attempt.workspace_id),
        attempt.id,
        EventPayload::AttemptSubagentBound(AttemptSubagentBound {
            task_id: task.id,
            task_key: task.key.clone(),
            agent_id: run.external_agent_id.clone(),
            agent_type: stats.agent_type.clone().or_else(|| run.agent_type.clone()),
            model: stats.model.clone(),
        }),
    )
    .with_attempt(Some(attempt.id))
}

/// The agent id of a subagent attempt: `subagent:<type>`, `unknown` without one.
pub fn subagent_agent_id(agent_type: Option<&str>) -> String {
    format!(
        "subagent:{}",
        agent_type
            .filter(|name| !name.is_empty())
            .unwrap_or("unknown")
    )
}

/// The agent type a transcript attempt is named after: the transcript's, else the run's.
pub fn transcript_agent_type<'a>(
    stats: &'a TranscriptStats,
    run: &'a SubagentRun,
) -> Option<&'a str> {
    stats
        .agent_type
        .as_deref()
        .filter(|name| !name.is_empty())
        .or(run.agent_type.as_deref())
}

/// When a transcript attempt started: the transcript's first record, else the run's start or stop, else `at`.
pub fn transcript_start(stats: &TranscriptStats, run: &SubagentRun, at: Timestamp) -> Timestamp {
    stats
        .started_at
        .as_deref()
        .and_then(|text| parse_utc(text).ok())
        .or(run.started_at)
        .or(run.stopped_at)
        .unwrap_or(at)
}

/// The telemetry of a child attempt a `SubagentStart` creates.
pub fn child_telemetry(agent_id: &str, agent_type: Option<&str>) -> Map<String, Value> {
    [
        ("external_agent_id".to_owned(), json!(agent_id)),
        ("agent_type".to_owned(), json!(agent_type)),
    ]
    .into_iter()
    .collect()
}

/// Whether a subagent stop finishes this child attempt: it has a parent and has not finished.
pub fn finishes_with_subagent(child: &AgentAttempt) -> bool {
    child.parent_attempt_id.is_some() && !is_terminal(child.status)
}

/// The number of a task's next attempt, given its existing attempts.
pub fn next_number(existing: &[AgentAttempt]) -> i64 {
    existing
        .iter()
        .map(|attempt| attempt.number)
        .max()
        .unwrap_or(0)
        + 1
}

/// A run's telemetry with the transcript's wall-time breakdown under `time`.
pub fn with_time(telemetry: &Map<String, Value>, stats: &TranscriptStats) -> Map<String, Value> {
    telemetry
        .iter()
        .filter(|(key, _)| key.as_str() != "time")
        .map(|(key, value)| (key.clone(), value.clone()))
        .chain(std::iter::once(("time".to_owned(), stats.time.clone())))
        .collect()
}

/// A session after an operator bound it to `task`.
pub fn bound_session(session: &AgentSession, task: &Task, now: Timestamp) -> AgentSession {
    AgentSession {
        workspace_id: Some(task.workspace_id),
        task_id: Some(task.id),
        binding_source: crate::binding::BindingSource::ExplicitBinding
            .as_str()
            .to_owned(),
        updated_at: now,
        ..session.clone()
    }
}

/// The `agent_session.bound` event of an operator binding.
pub fn bound_event(session_id: Option<&str>, cwd: Option<&str>) -> EventPayload {
    EventPayload::AgentSessionBound(AgentSessionBound {
        cwd: cwd.map(str::to_owned),
        session_id: session_id.map(str::to_owned),
    })
}

/// What ingesting one record did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestStatus {
    Ingested,
    /// The record was received before: nothing was written.
    Duplicate,
}

impl IngestStatus {
    /// The status text (`ingested` or `duplicate`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ingested => "ingested",
            Self::Duplicate => "duplicate",
        }
    }
}

/// Why a record could not be ingested: the Python exception class name and its message, which the
/// spooled record keeps as `spool_reason`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub type_name: String,
    pub message: String,
}

impl Failure {
    pub fn new(type_name: &str, message: impl Into<String>) -> Self {
        Self {
            type_name: type_name.to_owned(),
            message: message.into(),
        }
    }

    /// `f"{type}: {message}"[:500]`.
    pub fn reason(&self) -> String {
        crate::spool::failure_reason(&self.type_name, &self.message)
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.type_name, self.message)
    }
}

impl From<WgError> for Failure {
    fn from(error: WgError) -> Self {
        Self::new(error.code.as_str(), error.message)
    }
}
