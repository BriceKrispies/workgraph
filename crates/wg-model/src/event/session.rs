//! Payloads of the `agent_session.*` events (`integrations/claude_code/ingest.py`).

use serde::Serialize;
use serde_json::Value;
use wg_types::Id;

/// `agent_session.bound`. Written against a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentSessionBound {
    pub cwd: String,
    pub session_id: String,
}

/// `agent_session.started`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentSessionStarted {
    pub binding: Value,
    pub external_session_id: String,
    pub provider: String,
    pub source: String,
    pub task_id: Option<Id>,
}

/// `agent_session.ended`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentSessionEnded {
    pub reason: Option<String>,
    pub tool_calls: i64,
    pub tool_failures: i64,
}

/// `agent_session.internal_task_created` and `agent_session.internal_task_completed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentInternalTaskChanged {
    pub authoritative: bool,
    pub external_task_id: String,
    pub mapped_task_id: Option<Id>,
    pub subject: String,
}

/// `agent_session.subagent_started` and `agent_session.subagent_stopped`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentSubagentChanged {
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub child_attempt_id: Option<Id>,
}
