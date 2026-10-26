//! SQL text and parameters for the session family (SH-13): agent sessions, session bindings,
//! subagent runs, agent internal tasks, hook events and tool events, and the selections that
//! retention compaction makes.
//!
//! Like [`crate::sql`], every statement is a value ([`Statement`]); `wg-store` binds and runs it.
//! The typed rows here ([`SessionSummary`], [`HookEventCount`], [`StoppedRunOwner`]) are the
//! shapes the Python readers return or join into; they decode from the values a query returns.
//!
//! Ordering: where Python's query has no `ORDER BY`, the statement here orders by the closest
//! stable key and its doc comment says which one.

use wg_model::Table;
use wg_types::{AttemptStatus, ErrorCode, Id, TaskStatus, Timestamp, WgError};

use crate::codec::{Cols, Raw, id_sql, text_sql, ts_sql};
use crate::rows::columns;
use crate::sql::{Statement, quote};

/// Attempt states that count as open for subagent binding (`OPEN_ATTEMPT_STATUSES` in
/// `claude_code/ingest.py`).
pub const OPEN_ATTEMPT_STATUSES: [AttemptStatus; 3] = [
    AttemptStatus::Queued,
    AttemptStatus::Preparing,
    AttemptStatus::Running,
];

/// The quoted, comma-separated column list.
fn names_sql(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| quote(name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `SELECT <names> FROM <table> WHERE <clause> ORDER BY <order>`; the caller numbers the
/// placeholders in `clause` against `params`.
fn select_named(
    table: &str,
    names: &[&str],
    clause: &str,
    order: &str,
    params: Vec<Raw>,
) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE {clause} ORDER BY {order}",
            names_sql(names),
            quote(table),
        ),
        params,
    }
}

/// `SELECT <every column of table> ...` as [`select_named`].
fn select(table: Table, clause: &str, order: &str, params: Vec<Raw>) -> Statement {
    select_named(table.name(), columns(table), clause, order, params)
}

/// Appends `LIMIT ?n` when a limit is given (no limit otherwise).
fn with_limit(mut statement: Statement, limit: Option<i64>) -> Statement {
    if let Some(limit) = limit {
        statement.params.push(Raw::Int(limit));
        statement
            .sql
            .push_str(&format!(" LIMIT ?{}", statement.params.len()));
    }
    statement
}

/// `?first, ?first+1, ...` for `count` placeholders.
fn in_list(first: usize, count: usize) -> String {
    (first..first + count)
        .map(|n| format!("?{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn corrupt(what: &str) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("unexpected query result: {what}"),
    )
}

/// The one-column, one-row answer of a count, decoded.
pub fn count_from_rows(rows: &[Vec<Raw>]) -> Result<i64, WgError> {
    match rows.first().and_then(|row| row.first()) {
        Some(Raw::Int(count)) => Ok(*count),
        _ => Err(corrupt("count is not an integer")),
    }
}

/// The one-column answer of a `MAX` over a timestamp column; `None` when the table is empty.
pub fn optional_ts_from_rows(rows: &[Vec<Raw>]) -> Result<Option<Timestamp>, WgError> {
    match rows.first().and_then(|row| row.first()) {
        None | Some(Raw::Null) => Ok(None),
        Some(Raw::Text(text)) => Timestamp::parse_iso(text).map(Some),
        Some(_) => Err(corrupt("timestamp is not text")),
    }
}

/// The first column of each row as an id.
pub fn ids_from_rows(rows: &[Vec<Raw>]) -> Result<Vec<Id>, WgError> {
    rows.iter()
        .map(|row| match row.first() {
            Some(Raw::Text(text)) => wg_types::parse_id(text),
            _ => Err(corrupt("id is not text")),
        })
        .collect()
}

/// Columns of [`SessionSummary`], in the order its decoder reads them.
const SUMMARY_NAMES: &[&str] = &[
    "id",
    "external_session_id",
    "binding_source",
    "task_id",
    "attempt_id",
    "cwd",
    "model",
    "tool_calls",
    "tool_failures",
    "started_at",
    "ended_at",
    "end_reason",
];

/// The fields of an agent session that the session listing shows (`HookIngestor.sessions`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: Id,
    pub external_session_id: String,
    pub binding_source: String,
    pub task_id: Option<Id>,
    pub attempt_id: Option<Id>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub tool_calls: i64,
    pub tool_failures: i64,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub end_reason: Option<String>,
}

impl SessionSummary {
    /// Decodes a row selected with the summary's columns.
    pub fn from_raw(values: &[Raw]) -> Result<Self, WgError> {
        let c = Cols::new(SUMMARY_NAMES, values);
        Ok(SessionSummary {
            id: c.id("id")?,
            external_session_id: c.text("external_session_id")?,
            binding_source: c.text("binding_source")?,
            task_id: c.opt_id("task_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            cwd: c.opt_text("cwd")?,
            model: c.opt_text("model")?,
            tool_calls: c.int("tool_calls")?,
            tool_failures: c.int("tool_failures")?,
            started_at: c.opt_ts("started_at")?,
            ended_at: c.opt_ts("ended_at")?,
            end_reason: c.opt_text("end_reason")?,
        })
    }
}

/// One group of the hook-event statistics: the hook name and how many events carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEventCount {
    pub hook_event_name: String,
    pub count: i64,
}

impl HookEventCount {
    /// Decodes a row of `(hook_event_name, count)`.
    pub fn from_raw(values: &[Raw]) -> Result<Self, WgError> {
        const NAMES: &[&str] = &["hook_event_name", "count"];
        let c = Cols::new(NAMES, values);
        Ok(HookEventCount {
            hook_event_name: c.text("hook_event_name")?,
            count: c.int("count")?,
        })
    }
}

/// A stopped subagent run joined to the task it is linked to: the run's id, its task, and the
/// task's workspace and status. The join replaces `subagents.py`'s second query, which looked up
/// the owners of the runs' task ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoppedRunOwner {
    pub run_id: Id,
    pub task_id: Id,
    pub workspace_id: Id,
    pub status: TaskStatus,
}

impl StoppedRunOwner {
    /// Decodes a row of `(run_id, task_id, workspace_id, status)`.
    pub fn from_raw(values: &[Raw]) -> Result<Self, WgError> {
        const NAMES: &[&str] = &["run_id", "task_id", "workspace_id", "status"];
        let c = Cols::new(NAMES, values);
        Ok(StoppedRunOwner {
            run_id: c.id("run_id")?,
            task_id: c.id("task_id")?,
            workspace_id: c.id("workspace_id")?,
            status: c.parsed("status")?,
        })
    }
}

// ----- agent sessions ----------------------------------------------------------------------

/// The agent session of `provider` with `external_session_id` (unique per provider).
pub fn agent_session_by_external(provider: &str, external_session_id: &str) -> Statement {
    select(
        Table::AgentSession,
        "\"provider\" = ?1 AND \"external_session_id\" = ?2",
        "\"id\"",
        vec![text_sql(provider), text_sql(external_session_id)],
    )
}

/// A page of session summaries of `provider`, newest first, optionally narrowed to one workspace.
/// Order: `created_at` descending, then `id` descending (Python gives no tie break).
pub fn agent_session_summaries(
    provider: &str,
    workspace_id: Option<Id>,
    limit: i64,
    offset: i64,
) -> Statement {
    let mut clause = String::from("\"provider\" = ?1");
    let mut params = vec![text_sql(provider)];
    if let Some(workspace_id) = workspace_id {
        params.push(id_sql(workspace_id));
        clause.push_str(&format!(" AND \"workspace_id\" = ?{}", params.len()));
    }
    params.push(Raw::Int(limit));
    params.push(Raw::Int(offset));
    let limit_at = params.len() - 1;
    let mut statement = select_named(
        Table::AgentSession.name(),
        SUMMARY_NAMES,
        &clause,
        "\"created_at\" DESC, \"id\" DESC",
        params,
    );
    statement
        .sql
        .push_str(&format!(" LIMIT ?{limit_at} OFFSET ?{}", limit_at + 1));
    statement
}

/// The subagent run of one session with `external_agent_id` (unique per session).
pub fn subagent_run_by_external(session_id: Id, external_agent_id: &str) -> Statement {
    select(
        Table::SubagentRun,
        "\"session_id\" = ?1 AND \"external_agent_id\" = ?2",
        "\"id\"",
        vec![id_sql(session_id), text_sql(external_agent_id)],
    )
}

/// The number of subagent runs one session has started.
pub fn subagent_run_count_for_session(session_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT COUNT(*) FROM {} WHERE {} = ?1",
            quote(Table::SubagentRun.name()),
            quote("session_id"),
        ),
        params: vec![id_sql(session_id)],
    }
}

/// The agent internal task of a session by its external task id (unique per session).
pub fn internal_task_by_external(session_id: Id, external_task_id: &str) -> Statement {
    select(
        Table::AgentInternalTask,
        "\"session_id\" = ?1 AND \"external_task_id\" = ?2",
        "\"id\"",
        vec![id_sql(session_id), text_sql(external_task_id)],
    )
}

// ----- session bindings --------------------------------------------------------------------

/// The active binding for an external session id. Python takes the first row without an
/// `ORDER BY`; this takes the oldest by `created_at`, then `id`.
pub fn active_binding_by_external(external_session_id: &str) -> Statement {
    select(
        Table::SessionBinding,
        "\"active\" = 1 AND \"external_session_id\" = ?1",
        "\"created_at\", \"id\"",
        vec![text_sql(external_session_id)],
    )
}

/// The active bindings that carry a working-directory prefix. Order: `created_at`, `id`.
///
/// Python's `ingest._resolve_binding` takes the first matching prefix without an `ORDER BY`; the
/// resolution is deterministic here, the oldest matching binding winning.
pub fn active_cwd_bindings() -> Statement {
    select(
        Table::SessionBinding,
        "\"active\" = 1 AND \"cwd_prefix\" IS NOT NULL",
        "\"created_at\", \"id\"",
        Vec::new(),
    )
}

/// The active bindings an unbind names. A binding id narrows to that row; otherwise a cwd prefix
/// narrows to the bindings with that prefix; with neither, every active binding matches (the
/// precedence of Python's `HookIngestor.unbind`). Order: `created_at`, `id`.
pub fn active_bindings(binding_id: Option<Id>, cwd_prefix: Option<&str>) -> Statement {
    let mut clause = String::from("\"active\" = 1");
    let mut params = Vec::new();
    if let Some(binding_id) = binding_id {
        params.push(id_sql(binding_id));
        clause.push_str(&format!(" AND \"id\" = ?{}", params.len()));
    } else if let Some(prefix) = cwd_prefix {
        params.push(text_sql(prefix));
        clause.push_str(&format!(" AND \"cwd_prefix\" = ?{}", params.len()));
    }
    select(
        Table::SessionBinding,
        &clause,
        "\"created_at\", \"id\"",
        params,
    )
}

/// Marks the bindings with `ids` inactive (the write half of an unbind).
pub fn deactivate_bindings(ids: &[Id]) -> Statement {
    Statement {
        sql: format!(
            "UPDATE {} SET {} = 0 WHERE {} IN ({})",
            quote(Table::SessionBinding.name()),
            quote("active"),
            quote("id"),
            in_list(1, ids.len()),
        ),
        params: ids.iter().map(|id| id_sql(*id)).collect(),
    }
}

// ----- hook events and tool events ---------------------------------------------------------

/// The id of the hook event with `dedupe_key` (unique), if one was ingested.
pub fn hook_event_id_by_dedupe_key(dedupe_key: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE {} = ?1",
            quote("id"),
            quote(Table::HookEvent.name()),
            quote("dedupe_key"),
        ),
        params: vec![text_sql(dedupe_key)],
    }
}

/// The number of hook events.
pub fn hook_event_count() -> Statement {
    Statement {
        sql: format!("SELECT COUNT(*) FROM {}", quote(Table::HookEvent.name())),
        params: Vec::new(),
    }
}

/// The latest `received_at` of any hook event (one row, NULL when there are none).
pub fn hook_event_last_received() -> Statement {
    Statement {
        sql: format!(
            "SELECT MAX({}) FROM {}",
            quote("received_at"),
            quote(Table::HookEvent.name())
        ),
        params: Vec::new(),
    }
}

/// Hook events counted by `hook_event_name`. Order: the name.
pub fn hook_event_counts_by_name() -> Statement {
    Statement {
        sql: format!(
            "SELECT {name}, COUNT(*) AS \"count\" FROM {table} GROUP BY {name} ORDER BY {name}",
            name = quote("hook_event_name"),
            table = quote(Table::HookEvent.name()),
        ),
        params: Vec::new(),
    }
}

/// The tool events of one hook event. Order: `occurred_at`, `id`.
pub fn tool_events_for_hook(hook_event_id: Id) -> Statement {
    select(
        Table::ToolEvent,
        "\"hook_event_id\" = ?1",
        "\"occurred_at\", \"id\"",
        vec![id_sql(hook_event_id)],
    )
}

// ----- subagent runs -----------------------------------------------------------------------

fn stop_clause(stopped: bool) -> &'static str {
    if stopped {
        "\"stopped_at\" IS NOT NULL"
    } else {
        "\"stopped_at\" IS NULL"
    }
}

/// Subagent runs by whether they have stopped. Order: `id` (Python gives no order).
pub fn subagent_runs_by_stop(stopped: bool) -> Statement {
    select(
        Table::SubagentRun,
        stop_clause(stopped),
        "\"id\"",
        Vec::new(),
    )
}

/// The number of subagent runs that have (or have not) stopped.
pub fn subagent_run_count_by_stop(stopped: bool) -> Statement {
    Statement {
        sql: format!(
            "SELECT COUNT(*) FROM {} WHERE {}",
            quote(Table::SubagentRun.name()),
            stop_clause(stopped),
        ),
        params: Vec::new(),
    }
}

/// Stopped subagent runs joined to the task they are linked to (an inner join: a run without a
/// task is not an owner row). Order: the run's `id`.
pub fn stopped_run_owners() -> Statement {
    Statement {
        sql: format!(
            "SELECT {run}.{id} AS \"run_id\", {run}.{task} AS \"task_id\", \
             {tasks}.{ws} AS \"workspace_id\", {tasks}.{status} AS \"status\" \
             FROM {run} JOIN {tasks} ON {tasks}.{id} = {run}.{task} \
             WHERE {run}.{stopped} IS NOT NULL ORDER BY {run}.{id}",
            run = quote(Table::SubagentRun.name()),
            tasks = quote(Table::Task.name()),
            id = quote("id"),
            task = quote("task_id"),
            ws = quote("workspace_id"),
            status = quote("status"),
            stopped = quote("stopped_at"),
        ),
        params: Vec::new(),
    }
}

/// Subagent runs with a transcript but no model yet. Order: `id`, up to `limit`.
pub fn subagent_runs_needing_model(limit: Option<i64>) -> Statement {
    with_limit(
        select(
            Table::SubagentRun,
            "\"transcript_path\" IS NOT NULL AND \"model\" IS NULL",
            "\"id\"",
            Vec::new(),
        ),
        limit,
    )
}

/// Subagent runs with a transcript. Order: `id`, up to `limit`.
pub fn subagent_runs_with_transcript(limit: Option<i64>) -> Statement {
    with_limit(
        select(
            Table::SubagentRun,
            "\"transcript_path\" IS NOT NULL",
            "\"id\"",
            Vec::new(),
        ),
        limit,
    )
}

/// Subagent runs whose telemetry text contains `text`: a SQL `LIKE` prefilter, which the caller
/// follows with its own check of the `trial_ids` list. Order: most recently stopped first, nulls
/// last, then `id`.
pub fn subagent_runs_mentioning(text: &str) -> Statement {
    select(
        Table::SubagentRun,
        "CAST(\"telemetry\" AS TEXT) LIKE ?1",
        "\"stopped_at\" DESC NULLS LAST, \"id\"",
        vec![Raw::Text(format!("%{text}%"))],
    )
}

/// The newest open attempt of a task that no subagent run is bound to (one row). Open means one
/// of [`OPEN_ATTEMPT_STATUSES`]. Order: `number` descending.
pub fn open_attempt_for_subagent(task_id: Id) -> Statement {
    let mut params = vec![id_sql(task_id)];
    params.extend(
        OPEN_ATTEMPT_STATUSES
            .iter()
            .map(|status| text_sql(status.as_str())),
    );
    let clause = format!(
        "\"task_id\" = ?1 AND \"status\" IN ({}) AND \"id\" NOT IN \
         (SELECT \"attempt_id\" FROM {} WHERE \"attempt_id\" IS NOT NULL)",
        in_list(2, OPEN_ATTEMPT_STATUSES.len()),
        quote(Table::SubagentRun.name()),
    );
    with_limit(
        select(Table::AgentAttempt, &clause, "\"number\" DESC", params),
        Some(1),
    )
}

// ----- retention compaction ----------------------------------------------------------------

/// Hook events whose raw payload was received before `cutoff` and not yet purged, as their ids
/// (`jobs/retention.py`). Order: `received_at`, `id`.
pub fn hook_payloads_to_purge(cutoff: Timestamp) -> Statement {
    Statement {
        sql: format!(
            "SELECT {id} FROM {table} WHERE {received} < ?1 AND {payload} IS NOT NULL \
             AND {purged} IS NULL ORDER BY {received}, {id}",
            id = quote("id"),
            table = quote(Table::HookEvent.name()),
            received = quote("received_at"),
            payload = quote("payload"),
            purged = quote("payload_purged_at"),
        ),
        params: vec![ts_sql(cutoff)],
    }
}

/// Drops the payloads of the hook events with `ids` and stamps `payload_purged_at` with `at`.
pub fn purge_hook_payloads(ids: &[Id], at: Timestamp) -> Statement {
    let mut params = vec![ts_sql(at)];
    params.extend(ids.iter().map(|id| id_sql(*id)));
    Statement {
        sql: format!(
            "UPDATE {table} SET {payload} = NULL, {purged} = ?1 WHERE {id} IN ({marks})",
            table = quote(Table::HookEvent.name()),
            payload = quote("payload"),
            purged = quote("payload_purged_at"),
            id = quote("id"),
            marks = in_list(2, ids.len()),
        ),
        params,
    }
}

/// Telemetry artifacts created before `cutoff` and not yet purged. Order: `created_at`, `id`.
pub fn telemetry_artifacts_to_purge(cutoff: Timestamp) -> Statement {
    select(
        Table::Artifact,
        "\"retention\" = 'telemetry' AND \"created_at\" < ?1 AND \"purged_at\" IS NULL",
        "\"created_at\", \"id\"",
        vec![ts_sql(cutoff)],
    )
}

/// Stamps `purged_at` with `at` on the artifacts with `ids` (their files are removed by the shell).
pub fn mark_artifacts_purged(ids: &[Id], at: Timestamp) -> Statement {
    let mut params = vec![ts_sql(at)];
    params.extend(ids.iter().map(|id| id_sql(*id)));
    Statement {
        sql: format!(
            "UPDATE {table} SET {purged} = ?1 WHERE {id} IN ({marks})",
            table = quote(Table::Artifact.name()),
            purged = quote("purged_at"),
            id = quote("id"),
            marks = in_list(2, ids.len()),
        ),
        params,
    }
}
