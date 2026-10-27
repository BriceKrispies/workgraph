//! The decisions of the `agent_run` job (`RunService.execute` and `_EventRecorder` of
//! `orchestration/runs.py`): the attempt starting to prepare and to run, the agent's streamed
//! events recorded as attempt events, and the agent's session bound to the attempt.

use serde_json::{Map, Value};
use wg_agentspec::{AgentEvent, AgentRunResult, EventKind, RunStatus};
use wg_exec::attempt::task_after_prepare;
use wg_exec::run::{EVENT_BATCH_LIMIT, EVENT_MESSAGE_LIMIT, SyntheticResult, session_provider_key};
use wg_model::entity::{AgentAttempt, AgentSession, Artifact, JsonObject};
use wg_model::event::{
    AttemptMessage, AttemptProgress, AttemptResult, AttemptUsage, EventDraft, EventPayload,
};
use wg_model::stamp::{IdSeq, Stamp};
use wg_model::{Decision, Write};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::transition::{Change, transition};
use wg_types::{AttemptStatus, Id, TaskStatus, WgError};

use crate::spec::attempts::set_status;

/// The key a database session is bound under when the agent reports a session.
pub const BINDING_SOURCE: &str = "workgraph_launched";

/// The error for a job payload that lacks a member.
pub fn missing_payload(what: &str) -> WgError {
    WgError::validation(format!("job payload has no {what}"))
}

/// The result recorded for a run that never produced one: failed, with the reason in its
/// telemetry.
pub fn synthetic_result(found: &SyntheticResult) -> AgentRunResult {
    let mut result = AgentRunResult::new(RunStatus::Failed, None);
    result.error = Some(found.error.clone());
    result.telemetry.insert(
        "exit_reason".to_owned(),
        Value::String(found.exit_reason.to_owned()),
    );
    result
}

/// What the attempt records about where it ran.
pub fn environment(
    platform: &str,
    version: &str,
    provider_detail: &str,
    workdir: &str,
    workspace_dir: &str,
) -> Map<String, Value> {
    [
        ("platform", platform.to_owned()),
        ("runtime", format!("workgraph-rs {version}")),
        ("workgraph", version.to_owned()),
        ("provider_detail", provider_detail.to_owned()),
        ("workdir", workdir.to_owned()),
        ("workspace_dir", workspace_dir.to_owned()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), Value::String(value)))
    .collect()
}

/// The attempt is preparing, and its claimed task is running (`execute`, first transaction).
pub fn preparing_decision(
    world: &TaskWorld,
    attempt: &AgentAttempt,
    stamp: &Stamp,
) -> Result<Decision<()>, WgError> {
    let (event_id, ids) = stamp.ids().next();
    let base = match set_status(
        stamp,
        event_id,
        attempt,
        AttemptStatus::Preparing,
        Map::new(),
    )? {
        Some((row, event)) => Decision::new(()).write(Write::update(row)).event(event),
        None => Decision::new(()),
    };
    let task = world
        .tasks
        .task(attempt.task_id)
        .ok_or_else(|| WgError::not_found(format!("task {} not found", attempt.task_id)))?;
    if task.status != TaskStatus::Claimed {
        return Ok(base);
    }
    let reason = format!("attempt #{}", attempt.number);
    let moved = transition(
        &world.tasks,
        &world.stacks,
        stamp,
        ids,
        task.id,
        task_after_prepare(task.status),
        Change {
            reason: Some(&reason),
            ..Change::default()
        },
    )?;
    Ok(base.then(moved.decision))
}

/// The attempt is running: the prompt artifact and the environment are recorded and the status
/// changes (`execute`, second transaction).
pub fn running_decision(
    attempt: &AgentAttempt,
    artifact: Artifact,
    environment: JsonObject,
    stamp: &Stamp,
) -> Result<Decision<()>, WgError> {
    let recorded = AgentAttempt {
        prompt_artifact_id: Some(artifact.id),
        environment,
        updated_at: stamp.at,
        ..attempt.clone()
    };
    let (event_id, _) = stamp.ids().next();
    let base = Decision::new(()).write(Write::insert(artifact));
    Ok(
        match set_status(
            stamp,
            event_id,
            &recorded,
            AttemptStatus::Running,
            Map::new(),
        )? {
            Some((row, event)) => base.write(Write::update(row)).event(event),
            None => base.write(Write::update(recorded)),
        },
    )
}

/// `{k: v}` of the scalar members only (`str | int | float | bool | None`).
fn scalars(data: &Map<String, Value>) -> Map<String, Value> {
    data.iter()
        .filter(|(_, value)| !matches!(value, Value::Array(_) | Value::Object(_)))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn lifted_text(extra: &mut Map<String, Value>, key: &str) -> Option<String> {
    match extra.get(key) {
        Some(Value::String(text)) => {
            let found = text.clone();
            extra.remove(key);
            Some(found)
        }
        _ => None,
    }
}

fn lifted_int(extra: &mut Map<String, Value>, key: &str) -> Option<i64> {
    match extra.get(key).and_then(Value::as_i64) {
        Some(found) => {
            extra.remove(key);
            Some(found)
        }
        None => None,
    }
}

/// The payload of the event type `attempt.<kind>` for a streamed event: its message (cut to 2000
/// characters) and its scalar data.
pub fn event_payload(event: &AgentEvent) -> EventPayload {
    let mut extra = scalars(&event.data);
    let message = match extra.remove("message") {
        Some(Value::String(text)) => text,
        Some(other) => other.to_string(),
        None => event.message.chars().take(EVENT_MESSAGE_LIMIT).collect(),
    };
    match event.kind {
        EventKind::Progress => EventPayload::AttemptProgress(AttemptProgress {
            message,
            percent: lifted_int(&mut extra, "percent"),
            extra,
        }),
        EventKind::Result => EventPayload::AttemptResult(AttemptResult {
            message,
            status: lifted_text(&mut extra, "status"),
            summary: lifted_text(&mut extra, "summary"),
            kind: lifted_text(&mut extra, "type"),
            extra,
        }),
        EventKind::Usage => EventPayload::AttemptUsage(AttemptUsage {
            message,
            input_tokens: lifted_int(&mut extra, "input_tokens"),
            output_tokens: lifted_int(&mut extra, "output_tokens"),
            extra,
        }),
        EventKind::Session => EventPayload::AttemptSession(AttemptMessage { message, extra }),
        EventKind::Message => EventPayload::AttemptMessage(AttemptMessage { message, extra }),
        EventKind::ToolUse => EventPayload::AttemptToolUse(AttemptMessage { message, extra }),
        EventKind::ToolResult => EventPayload::AttemptToolResult(AttemptMessage { message, extra }),
        EventKind::Log => EventPayload::AttemptLog(AttemptMessage { message, extra }),
        EventKind::Artifact => EventPayload::AttemptArtifact(AttemptMessage { message, extra }),
    }
}

/// `_EventRecorder._write`: the last events of a batch as attempt events (at most
/// [`EVENT_BATCH_LIMIT`]), unless the attempt already recorded its limit. The reply is how many
/// were written.
pub fn events_decision(
    stamp: &Stamp,
    ids: IdSeq,
    workspace: Id,
    attempt: Id,
    batch: &[AgentEvent],
    recorded: usize,
) -> Decision<usize> {
    let count = wg_exec::run::events_to_write(batch.len(), recorded);
    let kept = &batch[batch.len().saturating_sub(EVENT_BATCH_LIMIT)..];
    let drafts: Vec<EventDraft> = kept
        .iter()
        .take(count)
        .scan(ids, |seq, event| {
            let (id, next) = seq.next();
            *seq = next;
            Some(
                EventDraft::new(stamp, id, Some(workspace), attempt, event_payload(event))
                    .with_attempt(Some(attempt)),
            )
        })
        .collect();
    let written = drafts.len();
    drafts
        .into_iter()
        .fold(Decision::new(written), |decision, draft| {
            decision.event(draft)
        })
}

/// `_EventRecorder._bind_session`: the agent's session row is created, or rebound to this attempt,
/// and the attempt records its session id. `existing` is the row of the provider and session id.
#[allow(clippy::too_many_arguments)]
pub fn bind_session_decision(
    stamp: &Stamp,
    attempt: &AgentAttempt,
    adapter: &str,
    provider: &str,
    session_id: &str,
    model: Option<&str>,
    existing: Option<&AgentSession>,
) -> Decision<()> {
    let key = session_provider_key(adapter, provider);
    let row = match existing {
        None => AgentSession {
            id: stamp.id(0),
            provider: key.to_owned(),
            external_session_id: session_id.to_owned(),
            workspace_id: Some(attempt.workspace_id),
            attempt_id: Some(attempt.id),
            task_id: Some(attempt.task_id),
            binding_source: BINDING_SOURCE.to_owned(),
            cwd: None,
            model: model.map(str::to_owned),
            transcript_path: None,
            started_at: Some(stamp.at),
            ended_at: None,
            end_reason: None,
            tool_calls: 0,
            tool_failures: 0,
            created_at: stamp.at,
            updated_at: stamp.at,
        },
        Some(found) => AgentSession {
            attempt_id: Some(attempt.id),
            task_id: Some(attempt.task_id),
            workspace_id: Some(attempt.workspace_id),
            binding_source: BINDING_SOURCE.to_owned(),
            updated_at: stamp.at,
            ..found.clone()
        },
    };
    let bound = AgentAttempt {
        session_id: Some(session_id.to_owned()),
        updated_at: stamp.at,
        ..attempt.clone()
    };
    let write = if existing.is_some() {
        Write::update(row)
    } else {
        Write::insert(row)
    };
    Decision::new(()).write(write).write(Write::update(bound))
}
