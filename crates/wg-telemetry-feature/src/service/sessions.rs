//! Session-family reads and writes on a [`Session`] (SH-13): agent sessions, session bindings,
//! subagent runs, agent internal tasks, hook and tool events, and the retention compaction
//! selections.
//!
//! The statements come from [`crate::spec::sql`]; this module runs them and decodes the
//! rows. Writes of whole entity rows go through [`Session::apply_write`] like every other family.

use crate::spec::sql::{self, HookEventCount, SessionSummary, StoppedRunOwner};
use wg_model::entity::{
    AgentAttempt, AgentInternalTask, AgentSession, Artifact, SessionBinding, SubagentRun, ToolEvent,
};
use wg_types::{Id, Timestamp, WgError};

use wg_store::Session;
use wg_store::query::{many, one};

/// The agent session of `provider` with `external_session_id`, if one is recorded.
pub fn agent_session_by_external(
    session: &mut Session<'_>,
    provider: &str,
    external_session_id: &str,
) -> Result<Option<AgentSession>, WgError> {
    one(
        session,
        &sql::agent_session_by_external(provider, external_session_id),
    )
}

/// One page of session summaries of `provider`, newest first, optionally in one workspace.
pub fn agent_session_summaries(
    session: &mut Session<'_>,
    provider: &str,
    workspace_id: Option<Id>,
    limit: i64,
    offset: i64,
) -> Result<Vec<SessionSummary>, WgError> {
    let statement = sql::agent_session_summaries(provider, workspace_id, limit, offset);
    session
        .rows(&statement)?
        .iter()
        .map(|values| SessionSummary::from_raw(values))
        .collect()
}

/// The subagent run of `session_id` with `external_agent_id`, if one is recorded.
pub fn subagent_run_by_external(
    session: &mut Session<'_>,
    session_id: Id,
    external_agent_id: &str,
) -> Result<Option<SubagentRun>, WgError> {
    one(
        session,
        &sql::subagent_run_by_external(session_id, external_agent_id),
    )
}

/// The number of subagent runs `session_id` has started.
pub fn subagent_run_count_for_session(
    session: &mut Session<'_>,
    session_id: Id,
) -> Result<i64, WgError> {
    let rows = session.rows(&sql::subagent_run_count_for_session(session_id))?;
    sql::count_from_rows(&rows)
}

/// The agent internal task of `session_id` with `external_task_id`, if one is recorded.
pub fn internal_task_by_external(
    session: &mut Session<'_>,
    session_id: Id,
    external_task_id: &str,
) -> Result<Option<AgentInternalTask>, WgError> {
    one(
        session,
        &sql::internal_task_by_external(session_id, external_task_id),
    )
}

/// The active binding for an external session id, if one exists (the oldest when several).
pub fn active_binding_by_external(
    session: &mut Session<'_>,
    external_session_id: &str,
) -> Result<Option<SessionBinding>, WgError> {
    one(
        session,
        &sql::active_binding_by_external(external_session_id),
    )
}

/// The active bindings that carry a working-directory prefix, oldest first.
pub fn active_cwd_bindings(session: &mut Session<'_>) -> Result<Vec<SessionBinding>, WgError> {
    many(session, &sql::active_cwd_bindings())
}

/// The active bindings an unbind names (see [`sql::active_bindings`]).
pub fn active_bindings(
    session: &mut Session<'_>,
    binding_id: Option<Id>,
    cwd_prefix: Option<&str>,
) -> Result<Vec<SessionBinding>, WgError> {
    many(session, &sql::active_bindings(binding_id, cwd_prefix))
}

/// Marks the bindings with `ids` inactive; returns how many rows changed.
pub fn deactivate_bindings(session: &mut Session<'_>, ids: &[Id]) -> Result<usize, WgError> {
    session.run(&sql::deactivate_bindings(ids))
}

/// The id of the hook event with `dedupe_key`, if one was ingested.
pub fn hook_event_id_by_dedupe_key(
    session: &mut Session<'_>,
    dedupe_key: &str,
) -> Result<Option<Id>, WgError> {
    let rows = session.rows(&sql::hook_event_id_by_dedupe_key(dedupe_key))?;
    Ok(sql::ids_from_rows(&rows)?.into_iter().next())
}

/// The number of hook events.
pub fn hook_event_count(session: &mut Session<'_>) -> Result<i64, WgError> {
    let rows = session.rows(&sql::hook_event_count())?;
    sql::count_from_rows(&rows)
}

/// The latest `received_at` of any hook event, if there is one.
pub fn hook_event_last_received(session: &mut Session<'_>) -> Result<Option<Timestamp>, WgError> {
    let rows = session.rows(&sql::hook_event_last_received())?;
    sql::optional_ts_from_rows(&rows)
}

/// Hook events counted by name, by name.
pub fn hook_event_counts_by_name(
    session: &mut Session<'_>,
) -> Result<Vec<HookEventCount>, WgError> {
    let rows = session.rows(&sql::hook_event_counts_by_name())?;
    rows.iter()
        .map(|values| HookEventCount::from_raw(values))
        .collect()
}

/// The tool events of one hook event, in `occurred_at` order.
pub fn tool_events_for_hook(
    session: &mut Session<'_>,
    hook_event_id: Id,
) -> Result<Vec<ToolEvent>, WgError> {
    many(session, &sql::tool_events_for_hook(hook_event_id))
}

/// The subagent runs that have stopped (`stopped = true`) or not, by `id`.
pub fn subagent_runs_by_stop(
    session: &mut Session<'_>,
    stopped: bool,
) -> Result<Vec<SubagentRun>, WgError> {
    many(session, &sql::subagent_runs_by_stop(stopped))
}

/// The number of subagent runs that have stopped (or not).
pub fn subagent_run_count_by_stop(
    session: &mut Session<'_>,
    stopped: bool,
) -> Result<i64, WgError> {
    let rows = session.rows(&sql::subagent_run_count_by_stop(stopped))?;
    sql::count_from_rows(&rows)
}

/// Stopped subagent runs joined to their owning task, by run `id`.
pub fn stopped_run_owners(session: &mut Session<'_>) -> Result<Vec<StoppedRunOwner>, WgError> {
    let rows = session.rows(&sql::stopped_run_owners())?;
    rows.iter()
        .map(|values| StoppedRunOwner::from_raw(values))
        .collect()
}

/// Subagent runs with a transcript but no model, by `id`, up to `limit` when given.
pub fn subagent_runs_needing_model(
    session: &mut Session<'_>,
    limit: Option<i64>,
) -> Result<Vec<SubagentRun>, WgError> {
    many(session, &sql::subagent_runs_needing_model(limit))
}

/// Subagent runs with a transcript, by `id`, up to `limit` when given.
pub fn subagent_runs_with_transcript(
    session: &mut Session<'_>,
    limit: Option<i64>,
) -> Result<Vec<SubagentRun>, WgError> {
    many(session, &sql::subagent_runs_with_transcript(limit))
}

/// Subagent runs whose telemetry text contains `text` (a prefilter; see the storecodec builder).
pub fn subagent_runs_mentioning(
    session: &mut Session<'_>,
    text: &str,
) -> Result<Vec<SubagentRun>, WgError> {
    many(session, &sql::subagent_runs_mentioning(text))
}

/// The newest open attempt of `task_id` that no subagent run is bound to, if there is one.
pub fn open_attempt_for_subagent(
    session: &mut Session<'_>,
    task_id: Id,
) -> Result<Option<AgentAttempt>, WgError> {
    one(session, &sql::open_attempt_for_subagent(task_id))
}

/// The ids of hook events whose payload is due for purge (received before `cutoff`, not yet
/// purged), in `received_at` order.
pub fn hook_payloads_to_purge(
    session: &mut Session<'_>,
    cutoff: Timestamp,
) -> Result<Vec<Id>, WgError> {
    let rows = session.rows(&sql::hook_payloads_to_purge(cutoff))?;
    sql::ids_from_rows(&rows)
}

/// Drops the payloads of the hook events with `ids`, stamping `at`; returns the rows changed.
/// An empty `ids` changes nothing.
pub fn purge_hook_payloads(
    session: &mut Session<'_>,
    ids: &[Id],
    at: Timestamp,
) -> Result<usize, WgError> {
    if ids.is_empty() {
        return Ok(0);
    }
    session.run(&sql::purge_hook_payloads(ids, at))
}

/// The telemetry artifacts due for purge (created before `cutoff`, not yet purged).
pub fn telemetry_artifacts_to_purge(
    session: &mut Session<'_>,
    cutoff: Timestamp,
) -> Result<Vec<Artifact>, WgError> {
    many(session, &sql::telemetry_artifacts_to_purge(cutoff))
}

/// Stamps `purged_at` on the artifacts with `ids`; returns the rows changed. An empty `ids`
/// changes nothing.
pub fn mark_artifacts_purged(
    session: &mut Session<'_>,
    ids: &[Id],
    at: Timestamp,
) -> Result<usize, WgError> {
    if ids.is_empty() {
        return Ok(0);
    }
    session.run(&sql::mark_artifacts_purged(ids, at))
}
