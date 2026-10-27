//! `GET /api/v1/claude/status`: the user-scope hook installation, the hook statistics and the
//! recent Claude Code sessions. The sessions are model rows; the installation and the statistics
//! are computed by their own services and arrive as plain values. All four shapes are dicts.

use wg_json::Value;
use wg_model::entity::session::AgentSession;
use wg_types::{Id, Timestamp};

use crate::convention::{counts_field, isoformat};
use crate::wire_dto;

wire_dto! {
    /// The status document, keys in Python order. `sessions` are already converted with [`session`].
    pub struct ClaudeStatus("ClaudeStatus")
    dict status(user: Value, hooks: Value, sessions: Vec<Value>) {
        user: Value = user,
        hooks: Value = hooks,
        sessions: Vec<Value> = sessions,
    }
}

wire_dto! {
    /// The installation status of one scope (`installer.status`).
    pub struct InstallerStatus<'a>("InstallerStatus")
    dict installer_status(
        scope: &'a str,
        settings: &'a str,
        exists: bool,
        error: Option<&'a str>,
        installed_events: &'a [String],
        missing_events: &'a [String],
        command_resolves: bool
    ) {
        scope: &'a str = scope,
        settings: &'a str = settings,
        exists: bool = exists,
        error: Option<&'a str> = error,
        installed_events: &'a [String] = installed_events,
        missing_events: &'a [String] = missing_events,
        command_resolves: bool = command_resolves,
    }
}

wire_dto! {
    /// The hook statistics (`hooks.stats`). `last_received_at` is a `datetime` the handler
    /// stringifies itself, so it prints in `isoformat()` form.
    pub struct HookStats<'a>("HookStats")
    dict hook_stats(
        hook_events: i64,
        last_received_at: Option<Timestamp>,
        by_event: &'a [(String, i64)],
        spooled: i64,
        spool_failed: i64
    ) {
        hook_events: i64 = hook_events,
        #[serde(serialize_with = "isoformat")]
        last_received_at: Option<Timestamp> = last_received_at,
        #[serde(serialize_with = "counts_field")]
        by_event: &'a [(String, i64)] = by_event,
        spooled: i64 = spooled,
        spool_failed: i64 = spool_failed,
    }
}

wire_dto! {
    /// One Claude Code session (`hooks.sessions`). Its `binding` is the row's `binding_source`; its
    /// timestamps are `isoformat()` strings, as the handler builds them.
    pub struct SessionOut<'a>("SessionOut") dict session(s: &'a AgentSession) {
        id: Id = s.id,
        external_session_id: &'a str = &s.external_session_id,
        binding: &'a str = &s.binding_source,
        task_id: Option<Id> = s.task_id,
        attempt_id: Option<Id> = s.attempt_id,
        cwd: Option<&'a str> = s.cwd.as_deref(),
        model: Option<&'a str> = s.model.as_deref(),
        tool_calls: i64 = s.tool_calls,
        tool_failures: i64 = s.tool_failures,
        #[serde(serialize_with = "isoformat")]
        started_at: Option<Timestamp> = s.started_at,
        #[serde(serialize_with = "isoformat")]
        ended_at: Option<Timestamp> = s.ended_at,
        end_reason: Option<&'a str> = s.end_reason.as_deref(),
    }
}
