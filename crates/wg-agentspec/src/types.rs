//! The agent provider interface types (`integrations/agents/base.py`).
//!
//! Telemetry honesty: providers fill `usage` and `cost_usd` only from values the agent actually
//! reported; otherwise they stay empty.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

/// How an attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl RunStatus {
    /// The wire name (`succeeded`, `failed`, `cancelled`, `timed_out`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}

/// What a provider can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCapabilities {
    pub streaming_progress: bool,
    pub usage_reporting: bool,
    pub cost_reporting: bool,
    pub session_resume: bool,
    pub isolated_workspace: bool,
    pub cancellation: bool,
    pub subagents: bool,
    pub lifecycle_hooks: bool,
}

impl Default for AgentCapabilities {
    fn default() -> Self {
        Self {
            streaming_progress: false,
            usage_reporting: false,
            cost_reporting: false,
            session_resume: false,
            isolated_workspace: true,
            cancellation: true,
            subagents: false,
            lifecycle_hooks: false,
        }
    }
}

impl AgentCapabilities {
    /// The capability map in declaration order.
    pub fn to_json(self) -> Value {
        json!({
            "streaming_progress": self.streaming_progress,
            "usage_reporting": self.usage_reporting,
            "cost_reporting": self.cost_reporting,
            "session_resume": self.session_resume,
            "isolated_workspace": self.isolated_workspace,
            "cancellation": self.cancellation,
            "subagents": self.subagents,
            "lifecycle_hooks": self.lifecycle_hooks,
        })
    }
}

/// One attempt to run.
#[derive(Debug, Clone)]
pub struct AgentRunRequest {
    pub attempt_id: String,
    pub task_id: String,
    pub task_key: String,
    pub prompt: String,
    pub workdir: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest: Value,
    pub timeout_seconds: f64,
    pub log_path: PathBuf,
    pub env: Vec<(String, String)>,
    pub model: Option<String>,
    pub resume_session_id: Option<String>,
    pub additional_dirs: Vec<PathBuf>,
}

/// The kind of a streamed event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Session,
    Progress,
    Message,
    ToolUse,
    ToolResult,
    Usage,
    Log,
    Artifact,
    Result,
}

impl EventKind {
    /// The wire name (`session`, `tool_use`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Progress => "progress",
            Self::Message => "message",
            Self::ToolUse => "tool_use",
            Self::ToolResult => "tool_result",
            Self::Usage => "usage",
            Self::Log => "log",
            Self::Artifact => "artifact",
            Self::Result => "result",
        }
    }
}

/// One structured event streamed while the agent runs.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentEvent {
    pub kind: EventKind,
    pub message: String,
    pub data: Map<String, Value>,
}

impl AgentEvent {
    /// An event without data.
    pub fn plain(kind: EventKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            data: Map::new(),
        }
    }

    /// An event with `data` (a JSON object; any other value gives an empty map).
    pub fn with_data(kind: EventKind, message: impl Into<String>, data: Value) -> Self {
        let data = match data {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        Self {
            kind,
            message: message.into(),
            data,
        }
    }
}

/// How an attempt ended, with what the agent reported.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentRunResult {
    pub status: RunStatus,
    pub exit_code: Option<i32>,
    pub summary: String,
    pub error: Option<String>,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub usage: Map<String, Value>,
    pub cost_usd: Option<f64>,
    pub provider_duration_ms: Option<i64>,
    pub num_turns: Option<i64>,
    pub artifacts: Vec<Value>,
    pub telemetry: Map<String, Value>,
}

impl AgentRunResult {
    /// A result with only a status and exit code; everything else empty.
    pub fn new(status: RunStatus, exit_code: Option<i32>) -> Self {
        Self {
            status,
            exit_code,
            summary: String::new(),
            error: None,
            session_id: None,
            model: None,
            usage: Map::new(),
            cost_usd: None,
            provider_duration_ms: None,
            num_turns: None,
            artifacts: Vec::new(),
            telemetry: Map::new(),
        }
    }
}
