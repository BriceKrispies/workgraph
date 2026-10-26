//! The session family: `agent_sessions`, `session_bindings`, `subagent_runs`,
//! `agent_internal_tasks`, `hook_events`, `tool_events` and `agent_profiles`.

use rpds::VectorSync;
use wg_types::{Id, Timestamp};

use crate::entity::JsonObject;
use crate::write::{Row, RowKey};

/// A row of `agent_sessions`: one external agent session (a Claude Code session, say).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSession {
    pub id: Id,
    pub provider: String,
    pub external_session_id: String,
    pub workspace_id: Option<Id>,
    pub attempt_id: Option<Id>,
    pub task_id: Option<Id>,
    /// How the session was bound to a task: `unbound` and the other binding sources.
    pub binding_source: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub transcript_path: Option<String>,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub end_reason: Option<String>,
    pub tool_calls: i64,
    pub tool_failures: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A row of `session_bindings`: a rule that binds external sessions to a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBinding {
    pub id: Id,
    pub workspace_id: Id,
    pub task_id: Id,
    pub provider: String,
    pub external_session_id: Option<String>,
    pub cwd_prefix: Option<String>,
    pub active: bool,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// A row of `subagent_runs`: one subagent a session started. Unique per session and external
/// agent id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRun {
    pub id: Id,
    pub session_id: Id,
    pub external_agent_id: String,
    pub agent_type: Option<String>,
    pub attempt_id: Option<Id>,
    pub started_at: Option<Timestamp>,
    pub stopped_at: Option<Timestamp>,
    pub last_message_excerpt: Option<String>,
    pub transcript_path: Option<String>,
    pub requested_model: Option<String>,
    pub model: Option<String>,
    pub usage: Option<JsonObject>,
    pub telemetry: Option<JsonObject>,
    pub task_id: Option<Id>,
}

/// A row of `agent_internal_tasks`: a task the agent keeps for itself inside a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInternalTask {
    pub id: Id,
    pub session_id: Id,
    pub external_task_id: String,
    pub subject: String,
    pub description: Option<String>,
    pub status: String,
    pub task_id: Option<Id>,
    pub external_agent_id: Option<String>,
    pub created_at: Timestamp,
    pub completed_at: Option<Timestamp>,
}

/// A row of `agent_profiles`: a named agent configuration of a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    pub id: Id,
    pub workspace_id: Id,
    pub name: String,
    pub adapter: Option<String>,
    pub model: Option<String>,
    /// JSON list of permission names.
    pub permissions: VectorSync<String>,
    pub enabled: bool,
    pub last_seen_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

/// A row of `hook_events`: one hook delivery from an agent provider. Unique per dedupe key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEvent {
    pub id: Id,
    pub dedupe_key: String,
    pub provider: String,
    pub hook_event_name: String,
    pub external_session_id: Option<String>,
    pub session_id: Option<Id>,
    pub attempt_id: Option<Id>,
    /// The raw hook payload, until retention purges it (`payload_purged_at`).
    pub payload: Option<JsonObject>,
    pub payload_purged_at: Option<Timestamp>,
    pub received_at: Timestamp,
    /// `direct` or `spool`.
    pub source: String,
}

/// A row of `tool_events`: one tool phase (`pre`, `success` or `failure`) of a hook event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEvent {
    pub id: Id,
    pub hook_event_id: Id,
    pub session_id: Option<Id>,
    pub attempt_id: Option<Id>,
    pub external_agent_id: Option<String>,
    pub phase: String,
    pub tool_name: String,
    pub tool_use_id: Option<String>,
    pub duration_ms: Option<i64>,
    pub error_excerpt: Option<String>,
    pub summary: JsonObject,
    pub occurred_at: Timestamp,
}

impl AgentSession {
    pub fn row_key(&self) -> RowKey {
        RowKey::AgentSession(self.id)
    }
}

impl SessionBinding {
    pub fn row_key(&self) -> RowKey {
        RowKey::SessionBinding(self.id)
    }
}

impl SubagentRun {
    pub fn row_key(&self) -> RowKey {
        RowKey::SubagentRun(self.id)
    }
}

impl AgentInternalTask {
    pub fn row_key(&self) -> RowKey {
        RowKey::AgentInternalTask(self.id)
    }
}

impl AgentProfile {
    pub fn row_key(&self) -> RowKey {
        RowKey::AgentProfile(self.id)
    }
}

impl HookEvent {
    pub fn row_key(&self) -> RowKey {
        RowKey::HookEvent(self.id)
    }
}

impl ToolEvent {
    pub fn row_key(&self) -> RowKey {
        RowKey::ToolEvent(self.id)
    }
}

impl From<AgentSession> for Row {
    fn from(row: AgentSession) -> Self {
        Row::AgentSession(row)
    }
}

impl From<SessionBinding> for Row {
    fn from(row: SessionBinding) -> Self {
        Row::SessionBinding(row)
    }
}

impl From<SubagentRun> for Row {
    fn from(row: SubagentRun) -> Self {
        Row::SubagentRun(row)
    }
}

impl From<AgentInternalTask> for Row {
    fn from(row: AgentInternalTask) -> Self {
        Row::AgentInternalTask(row)
    }
}

impl From<AgentProfile> for Row {
    fn from(row: AgentProfile) -> Self {
        Row::AgentProfile(row)
    }
}

impl From<HookEvent> for Row {
    fn from(row: HookEvent) -> Self {
        Row::HookEvent(row)
    }
}

impl From<ToolEvent> for Row {
    fn from(row: ToolEvent) -> Self {
        Row::ToolEvent(row)
    }
}
