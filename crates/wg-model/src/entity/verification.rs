//! The verification family: `verification_configs`, `verification_runs`,
//! `verification_check_results` and `task_acceptances`.

use rpds::VectorSync;
use wg_types::{Id, Timestamp};

use crate::entity::JsonObject;
use crate::write::{Row, RowKey};

/// A row of `verification_configs`: the checks a scope runs before its tasks are accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationConfig {
    pub id: Id,
    pub workspace_id: Id,
    /// `workspace`, `project`, `repository` or `task`.
    pub scope_type: String,
    pub scope_id: Id,
    pub name: String,
    /// JSON list of check objects.
    pub checks: VectorSync<JsonObject>,
    pub enabled: bool,
    pub approved_by: String,
    pub version: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A row of `verification_runs`: one run of a task's checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationRun {
    pub id: Id,
    pub workspace_id: Id,
    pub task_id: Id,
    pub attempt_id: Option<Id>,
    pub manifest_id: Option<Id>,
    /// JSON list of the check objects the run was started with.
    pub config_snapshot: VectorSync<JsonObject>,
    pub status: String,
    pub required_passed: bool,
    pub triggered_by: String,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

/// A row of `verification_check_results`: the outcome of one check of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationCheckResult {
    pub id: Id,
    pub run_id: Id,
    pub name: String,
    pub kind: String,
    pub required: bool,
    pub repository_id: Option<Id>,
    pub status: String,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub timed_out: bool,
    pub log_artifact_id: Option<Id>,
    pub output_excerpt: String,
    /// JSON list of failure signature strings.
    pub failure_signatures: VectorSync<String>,
    pub created_at: Timestamp,
}

/// A row of `task_acceptances`: an accept or reject decision on a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskAcceptance {
    pub id: Id,
    pub task_id: Id,
    pub attempt_id: Option<Id>,
    pub verification_run_id: Option<Id>,
    /// `accepted` or `rejected`.
    pub decision: String,
    /// `automated` or `manual`.
    pub mode: String,
    pub reason: String,
    pub actor: String,
    pub manifest_id: Option<Id>,
    pub created_at: Timestamp,
}

impl VerificationConfig {
    pub fn row_key(&self) -> RowKey {
        RowKey::VerificationConfig(self.id)
    }
}

impl VerificationRun {
    pub fn row_key(&self) -> RowKey {
        RowKey::VerificationRun(self.id)
    }
}

impl VerificationCheckResult {
    pub fn row_key(&self) -> RowKey {
        RowKey::VerificationCheckResult(self.id)
    }
}

impl TaskAcceptance {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskAcceptance(self.id)
    }
}

impl From<VerificationConfig> for Row {
    fn from(row: VerificationConfig) -> Self {
        Row::VerificationConfig(row)
    }
}

impl From<VerificationRun> for Row {
    fn from(row: VerificationRun) -> Self {
        Row::VerificationRun(row)
    }
}

impl From<VerificationCheckResult> for Row {
    fn from(row: VerificationCheckResult) -> Self {
        Row::VerificationCheckResult(row)
    }
}

impl From<TaskAcceptance> for Row {
    fn from(row: TaskAcceptance) -> Self {
        Row::TaskAcceptance(row)
    }
}
