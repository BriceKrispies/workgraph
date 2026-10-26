//! [`TelemetrySlice`]: agent sessions, their bindings, subagent runs, internal tasks, hook and tool
//! events, and agent profiles.

use rpds::RedBlackTreeMapSync;
use wg_types::Id;

use crate::entity::{
    AgentInternalTask, AgentProfile, AgentSession, HookEvent, SessionBinding, SubagentRun,
    ToolEvent,
};
use crate::write::{Row, RowKey, Write};

/// Loaded rows of the session family, keyed by primary key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TelemetrySlice {
    pub sessions: RedBlackTreeMapSync<Id, AgentSession>,
    pub bindings: RedBlackTreeMapSync<Id, SessionBinding>,
    pub subagent_runs: RedBlackTreeMapSync<Id, SubagentRun>,
    pub internal_tasks: RedBlackTreeMapSync<Id, AgentInternalTask>,
    pub hook_events: RedBlackTreeMapSync<Id, HookEvent>,
    pub tool_events: RedBlackTreeMapSync<Id, ToolEvent>,
    pub profiles: RedBlackTreeMapSync<Id, AgentProfile>,
}

impl TelemetrySlice {
    pub fn session(&self, id: Id) -> Option<&AgentSession> {
        self.sessions.get(&id)
    }

    /// The subagent runs of a session, by run id.
    pub fn subagent_runs_of(&self, session_id: Id) -> impl Iterator<Item = &SubagentRun> {
        self.subagent_runs
            .values()
            .filter(move |row| row.session_id == session_id)
    }

    /// The internal tasks of a session, by internal task id.
    pub fn internal_tasks_of(&self, session_id: Id) -> impl Iterator<Item = &AgentInternalTask> {
        self.internal_tasks
            .values()
            .filter(move |row| row.session_id == session_id)
    }

    /// The hook event with this dedupe key, if one is loaded.
    pub fn hook_event_by_dedupe_key(&self, dedupe_key: &str) -> Option<&HookEvent> {
        self.hook_events
            .values()
            .find(|row| row.dedupe_key == dedupe_key)
    }

    /// The slice after `write`. Writes to tables this slice does not hold leave it unchanged.
    #[must_use]
    pub fn apply(&self, write: &Write) -> Self {
        match write {
            Write::Insert(row) | Write::Update { row, .. } => match row {
                Row::AgentSession(row) => TelemetrySlice {
                    sessions: self.sessions.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::SessionBinding(row) => TelemetrySlice {
                    bindings: self.bindings.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::SubagentRun(row) => TelemetrySlice {
                    subagent_runs: self.subagent_runs.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::AgentInternalTask(row) => TelemetrySlice {
                    internal_tasks: self.internal_tasks.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::HookEvent(row) => TelemetrySlice {
                    hook_events: self.hook_events.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::ToolEvent(row) => TelemetrySlice {
                    tool_events: self.tool_events.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::AgentProfile(row) => TelemetrySlice {
                    profiles: self.profiles.insert(row.id, row.clone()),
                    ..self.clone()
                },
                _ => self.clone(),
            },
            Write::Delete(key) => match key {
                RowKey::AgentSession(id) => TelemetrySlice {
                    sessions: self.sessions.remove(id),
                    ..self.clone()
                },
                RowKey::SessionBinding(id) => TelemetrySlice {
                    bindings: self.bindings.remove(id),
                    ..self.clone()
                },
                RowKey::SubagentRun(id) => TelemetrySlice {
                    subagent_runs: self.subagent_runs.remove(id),
                    ..self.clone()
                },
                RowKey::AgentInternalTask(id) => TelemetrySlice {
                    internal_tasks: self.internal_tasks.remove(id),
                    ..self.clone()
                },
                RowKey::HookEvent(id) => TelemetrySlice {
                    hook_events: self.hook_events.remove(id),
                    ..self.clone()
                },
                RowKey::ToolEvent(id) => TelemetrySlice {
                    tool_events: self.tool_events.remove(id),
                    ..self.clone()
                },
                RowKey::AgentProfile(id) => TelemetrySlice {
                    profiles: self.profiles.remove(id),
                    ..self.clone()
                },
                _ => self.clone(),
            },
        }
    }
}
