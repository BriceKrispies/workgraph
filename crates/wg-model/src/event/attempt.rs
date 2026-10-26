//! Payloads of the `attempt.*` events (`core/services/attempts.py`, the worktree and transcript
//! ingestion services, and the run orchestration).
//!
//! Keys that a caller may leave out are `Option<_>` with `skip_serializing_if`; open payloads
//! (`data_open` in `contracts/events.json`) end with a flattened `extra`.

use rpds::VectorSync;
use serde::Serialize;
use wg_types::{AttemptStatus, Id, TaskKey};

use crate::entity::JsonObject;
use crate::event::{StrMap, ser};

/// `attempt.created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptCreated {
    pub task_id: Id,
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    pub number: i64,
    pub provider: String,
    pub adapter: String,
    pub model: Option<String>,
    pub status: AttemptStatus,
    pub parent_attempt_id: Option<Id>,
    pub retry_of_attempt_id: Option<Id>,
    pub rerun_of_attempt_id: Option<Id>,
}

/// `attempt.status_changed` (open: callers add `reason` and similar keys).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptStatusChanged {
    pub from: AttemptStatus,
    pub to: AttemptStatus,
    pub task_id: Id,
    #[serde(flatten)]
    pub extra: JsonObject,
}

/// `attempt.cancel_requested`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptCancelRequested {
    pub reason: Option<String>,
}

/// `attempt.succeeded`, `attempt.failed` and `attempt.cancelled`: the terminal transition of an
/// attempt, with its exit details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptFinished {
    pub task_id: Id,
    pub from: AttemptStatus,
    pub status: AttemptStatus,
    pub exit_reason: Option<String>,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub error: Option<String>,
}

/// `attempt.workspace_prepared`: the repositories and bases an attempt's workspace starts from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptWorkspacePrepared {
    pub repositories: VectorSync<String>,
    /// Repository name to base revision.
    pub bases: StrMap,
    pub isolation: String,
    pub baseline_manifest_id: Id,
    pub change_set_id: Option<Id>,
}

/// `attempt.outcome_captured`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptOutcomeCaptured {
    pub outcome_manifest_id: Id,
    pub modified_repositories: VectorSync<String>,
    pub read_only_violations: VectorSync<String>,
    /// Repository name to the number of commits the attempt made there.
    pub commits: JsonObject,
}

/// `attempt.subagent_bound`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptSubagentBound {
    pub task_id: Id,
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    pub agent_id: String,
    pub agent_type: Option<String>,
    pub model: Option<String>,
}

/// `attempt.artifact`, `attempt.log`, `attempt.message`, `attempt.session`, `attempt.tool_use`
/// and `attempt.tool_result`: a message attached to an attempt (open).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptMessage {
    pub message: String,
    #[serde(flatten)]
    pub extra: JsonObject,
}

/// `attempt.progress` (open; `percent` is optional).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptProgress {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<i64>,
    #[serde(flatten)]
    pub extra: JsonObject,
}

/// `attempt.result` (open; `status`, `summary` and `type` are optional).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptResult {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(flatten)]
    pub extra: JsonObject,
}

/// `attempt.usage` (open; token counts are optional).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttemptUsage {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i64>,
    #[serde(flatten)]
    pub extra: JsonObject,
}
