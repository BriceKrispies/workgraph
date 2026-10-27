//! `GET /api/v1/claude/status`: the user-scope hook installation, the hook statistics and the
//! recent Claude Code sessions. The sessions are model rows; the installation and the statistics
//! are computed by their own services and arrive as plain values.

use wg_json::{PyStr, Value, dict};
use wg_model::entity::session::AgentSession;
use wg_types::Timestamp;

use crate::convention::{TimeFormat, id, optional_timestamp, text};

/// The keys of the status document, in Python order.
pub const FIELDS: [&str; 3] = ["user", "hooks", "sessions"];

/// The keys of the installation status (`installer.status`).
pub const INSTALLER_FIELDS: [&str; 7] = [
    "scope",
    "settings",
    "exists",
    "error",
    "installed_events",
    "missing_events",
    "command_resolves",
];

/// The keys of the hook statistics (`hooks.stats`).
pub const HOOK_FIELDS: [&str; 5] = [
    "hook_events",
    "last_received_at",
    "by_event",
    "spooled",
    "spool_failed",
];

/// The keys of one session (`hooks.sessions`).
pub const SESSION_FIELDS: [&str; 12] = [
    "id",
    "external_session_id",
    "binding",
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

/// The status document. `sessions` are already converted with [`session`].
pub fn status(user: Value, hooks: Value, sessions: Vec<Value>) -> Value {
    dict!({
        "user": user,
        "hooks": hooks,
        "sessions": Value::List(sessions),
    })
}

/// The installation status of one scope.
#[allow(clippy::too_many_arguments)]
pub fn installer_status(
    scope: &str,
    settings: &str,
    exists: bool,
    error: Option<&str>,
    installed_events: &[String],
    missing_events: &[String],
    command_resolves: bool,
) -> Value {
    dict!({
        "scope": text(scope),
        "settings": text(settings),
        "exists": Value::Bool(exists),
        "error": error.map_or(Value::Null, text),
        "installed_events": strings(installed_events),
        "missing_events": strings(missing_events),
        "command_resolves": Value::Bool(command_resolves),
    })
}

/// The hook statistics. `last_received_at` is a `datetime` the handler stringifies itself, so it
/// prints in `isoformat()` form.
pub fn hook_stats(
    hook_events: i64,
    last_received_at: Option<Timestamp>,
    by_event: &[(String, i64)],
    spooled: i64,
    spool_failed: i64,
) -> Value {
    dict!({
        "hook_events": Value::Int(i128::from(hook_events)),
        "last_received_at": optional_timestamp(last_received_at, TimeFormat::Isoformat),
        "by_event": Value::Dict(
            by_event
                .iter()
                .map(|(name, count)| {
                    (PyStr::from(name.as_str()), Value::Int(i128::from(*count)))
                })
                .collect(),
        ),
        "spooled": Value::Int(i128::from(spooled)),
        "spool_failed": Value::Int(i128::from(spool_failed)),
    })
}

/// One Claude Code session. Its `binding` is the row's `binding_source`; its timestamps are
/// `isoformat()` strings, as the handler builds them.
pub fn session(row: &AgentSession) -> Value {
    dict!({
        "id": id(row.id),
        "external_session_id": text(&row.external_session_id),
        "binding": text(&row.binding_source),
        "task_id": row.task_id.map_or(Value::Null, id),
        "attempt_id": row.attempt_id.map_or(Value::Null, id),
        "cwd": row.cwd.as_deref().map_or(Value::Null, text),
        "model": row.model.as_deref().map_or(Value::Null, text),
        "tool_calls": Value::Int(i128::from(row.tool_calls)),
        "tool_failures": Value::Int(i128::from(row.tool_failures)),
        "started_at": optional_timestamp(row.started_at, TimeFormat::Isoformat),
        "ended_at": optional_timestamp(row.ended_at, TimeFormat::Isoformat),
        "end_reason": row.end_reason.as_deref().map_or(Value::Null, text),
    })
}

fn strings(items: &[String]) -> Value {
    Value::List(items.iter().map(|item| text(item)).collect())
}
