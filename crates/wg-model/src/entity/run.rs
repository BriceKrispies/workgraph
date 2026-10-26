//! The `run` family: `agent_attempts`, `revision_manifests`, `managed_worktrees`,
//! `resource_leases`, `change_sets`, `change_set_items` and `artifacts`.
//!
//! An attempt is one agent run of a task; its manifests, worktrees, change set and artifacts
//! belong to it. A resource lease has no foreign key: it is keyed by its resource and held by a
//! named holder, so it sits in this family because the run flow is what acquires it.

use rpds::VectorSync;
use wg_types::{
    AttemptStatus, ChangeSetItemStatus, ChangeSetStatus, Id, Timestamp, WorktreeStatus,
};

use crate::entity::{JsonObject, Real};
use crate::write::{Row, RowKey};

/// A row of `agent_attempts`: one agent run of a task (`number` counts per task).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAttempt {
    pub id: Id,
    pub workspace_id: Id,
    pub task_id: Id,
    pub number: i64,
    pub status: AttemptStatus,
    pub provider: String,
    pub adapter: String,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub parent_attempt_id: Option<Id>,
    pub retry_of_attempt_id: Option<Id>,
    pub rerun_of_attempt_id: Option<Id>,
    pub created_by: String,
    pub created_at: Timestamp,
    pub queued_at: Option<Timestamp>,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub queue_wait_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub exit_reason: Option<String>,
    pub exit_code: Option<i64>,
    pub error: Option<String>,
    pub summary: Option<String>,
    pub input_task_version: Option<i64>,
    pub input_task_snapshot: JsonObject,
    pub prompt_artifact_id: Option<Id>,
    pub config_snapshot: JsonObject,
    pub environment: JsonObject,
    pub baseline_manifest_id: Option<Id>,
    pub outcome_manifest_id: Option<Id>,
    pub usage: JsonObject,
    pub usage_source: Option<String>,
    pub cost_usd: Option<Real>,
    pub cost_source: Option<String>,
    pub telemetry: JsonObject,
    /// JSON list of repository ids.
    pub modified_repository_ids: VectorSync<String>,
    pub retries: i64,
    pub cancel_requested: bool,
    pub updated_at: Timestamp,
}

/// A row of `revision_manifests`: the revision of each repository at one point of an attempt
/// (`baseline` before the run, `outcome` after it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionManifest {
    pub id: Id,
    pub workspace_id: Id,
    pub attempt_id: Option<Id>,
    /// `baseline` or `outcome`.
    pub kind: String,
    /// JSON list of entry objects, one per repository.
    pub entries: VectorSync<JsonObject>,
    pub created_at: Timestamp,
}

/// A row of `managed_worktrees`: a worktree the system created for an attempt or a stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorktree {
    pub id: Id,
    pub workspace_id: Id,
    pub repository_id: Id,
    pub source_checkout_id: Id,
    pub attempt_id: Option<Id>,
    pub path: String,
    pub branch: String,
    pub base_sha: String,
    pub status: WorktreeStatus,
    pub last_error: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub removed_at: Option<Timestamp>,
}

/// A row of `resource_leases`: a named resource held by one holder. Keyed by the resource key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceLease {
    pub resource_key: String,
    pub holder: String,
    pub purpose: String,
    pub acquired_at: Timestamp,
    pub expires_at: Option<Timestamp>,
}

/// A row of `change_sets`: the commits one attempt made for one task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    pub id: Id,
    pub workspace_id: Id,
    pub task_id: Id,
    pub attempt_id: Option<Id>,
    pub title: String,
    pub status: ChangeSetStatus,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A row of `change_set_items`: one repository's part of a change set (unique per repository).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSetItem {
    pub id: Id,
    pub change_set_id: Id,
    pub repository_id: Id,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub branch: Option<String>,
    /// JSON list of commit shas.
    pub commit_shas: VectorSync<String>,
    pub status: ChangeSetItemStatus,
    pub detail: Option<String>,
    pub updated_at: Timestamp,
}

/// A row of `artifacts`: a file the system keeps for a task or an attempt (prompts, logs,
/// reports). The bytes live at `path`; the row holds their digest and retention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub id: Id,
    pub workspace_id: Id,
    pub task_id: Option<Id>,
    pub attempt_id: Option<Id>,
    pub kind: String,
    pub name: String,
    pub media_type: String,
    pub path: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub retention: String,
    pub purged_at: Option<Timestamp>,
    pub metadata: JsonObject,
    pub created_at: Timestamp,
}

impl AgentAttempt {
    pub fn row_key(&self) -> RowKey {
        RowKey::AgentAttempt(self.id)
    }
}

impl RevisionManifest {
    pub fn row_key(&self) -> RowKey {
        RowKey::RevisionManifest(self.id)
    }
}

impl ManagedWorktree {
    pub fn row_key(&self) -> RowKey {
        RowKey::ManagedWorktree(self.id)
    }
}

impl ResourceLease {
    pub fn row_key(&self) -> RowKey {
        RowKey::ResourceLease(self.resource_key.clone())
    }
}

impl ChangeSet {
    pub fn row_key(&self) -> RowKey {
        RowKey::ChangeSet(self.id)
    }
}

impl ChangeSetItem {
    pub fn row_key(&self) -> RowKey {
        RowKey::ChangeSetItem(self.id)
    }
}

impl Artifact {
    pub fn row_key(&self) -> RowKey {
        RowKey::Artifact(self.id)
    }
}

impl From<AgentAttempt> for Row {
    fn from(row: AgentAttempt) -> Self {
        Row::AgentAttempt(row)
    }
}

impl From<RevisionManifest> for Row {
    fn from(row: RevisionManifest) -> Self {
        Row::RevisionManifest(row)
    }
}

impl From<ManagedWorktree> for Row {
    fn from(row: ManagedWorktree) -> Self {
        Row::ManagedWorktree(row)
    }
}

impl From<ResourceLease> for Row {
    fn from(row: ResourceLease) -> Self {
        Row::ResourceLease(row)
    }
}

impl From<ChangeSet> for Row {
    fn from(row: ChangeSet) -> Self {
        Row::ChangeSet(row)
    }
}

impl From<ChangeSetItem> for Row {
    fn from(row: ChangeSetItem) -> Self {
        Row::ChangeSetItem(row)
    }
}

impl From<Artifact> for Row {
    fn from(row: Artifact) -> Self {
        Row::Artifact(row)
    }
}
