//! Payloads of the `task.*` events (`core/services/tasks.py`, verification, git, trackers,
//! runs and the scheduler).

use rpds::VectorSync;
use serde::Serialize;
use wg_types::{CommentKind, Id, RelationType, TaskKey, TaskPriority, TaskStatus, Timestamp};

use crate::entity::JsonObject;
use crate::event::{Changes, ser};

/// `task.created` (schema version 2: `tags` is a list, `acceptance_criteria` present).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskCreated {
    #[serde(serialize_with = "ser::task_key")]
    pub key: TaskKey,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: VectorSync<String>,
    /// Always `backlog`: a task created ready gets a `task.status_changed` after this.
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub tags: VectorSync<String>,
    pub project_ids: VectorSync<Id>,
    pub repository_ids: VectorSync<Id>,
    pub parent_id: Option<Id>,
    pub depends_on: VectorSync<Id>,
    pub assignee: Option<String>,
    pub auto_execute: bool,
}

/// `task.updated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskUpdated {
    #[serde(serialize_with = "ser::task_key")]
    pub key: TaskKey,
    pub changes: Changes,
    /// Whether the title or description changed after normalisation.
    pub material_change: bool,
    pub reason: Option<String>,
    pub version: i64,
}

/// `task.status_changed`. Callers may add `attempt_id` or `run_id` (`None`: key absent;
/// `Some(None)`: written as null) and further keys (`extra`, merged last, replacing a key of the
/// same name in place).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskStatusChanged {
    pub from: TaskStatus,
    pub to: TaskStatus,
    pub reason: Option<String>,
    pub version: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<Option<Id>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Option<Id>>,
    #[serde(flatten)]
    pub extra: JsonObject,
}

/// `task.reopened`: a transition out of `accepted` or `cancelled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskReopened {
    pub from: TaskStatus,
    pub to: TaskStatus,
    pub reason: Option<String>,
}

/// `task.became_ready`, `task.archived` and `task.unarchived`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskKeyed {
    #[serde(serialize_with = "ser::task_key")]
    pub key: TaskKey,
}

/// `task.claimed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskClaimed {
    pub claimed_by: String,
    /// Written with `isoformat()`.
    #[serde(serialize_with = "ser::isoformat")]
    pub expires_at: Timestamp,
}

/// `task.released`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskReleased {
    pub previous_claimant: Option<String>,
    pub reason: Option<String>,
}

/// `task.completed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskCompleted {
    /// The first 5000 characters of the summary.
    pub summary: String,
    pub next_status: TaskStatus,
    pub verification_configured: bool,
}

/// `task.comment_added`, `task.progress_reported` and `task.result_reported`. The comment's own
/// `data` object is merged last (`extra`), replacing a key of the same name in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskCommentRecorded {
    pub comment_id: Id,
    pub kind: CommentKind,
    /// The first 500 characters of the body.
    pub excerpt: String,
    #[serde(flatten)]
    pub extra: JsonObject,
}

/// `task.dependency_added` and `task.dependency_removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskDependencyChanged {
    pub depends_on: Id,
    #[serde(serialize_with = "ser::task_key")]
    pub depends_on_key: TaskKey,
}

/// `task.relation_added`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRelationAdded {
    pub target: Id,
    #[serde(serialize_with = "ser::task_key")]
    pub target_key: TaskKey,
    pub relation_type: RelationType,
}

/// `task.repository_linked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRepositoryLinked {
    pub repository_id: Id,
    /// The repository's name.
    pub repository: String,
    pub access: String,
}

/// `task.repository_unlinked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRepositoryUnlinked {
    pub repository_id: Id,
    pub repository: String,
}

/// `task.accepted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskAccepted {
    /// `automated`, `manual` or `external`.
    pub mode: String,
    pub reason: String,
    pub verification_run_id: Option<Id>,
    pub manifest_id: Option<Id>,
}

/// `task.rejected`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRejected {
    pub reason: String,
}

/// `task.escalated`: the attempt limit was reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskEscalated {
    pub failed_attempts: i64,
    pub limit: i64,
    pub reason: String,
}

/// `task.decomposed`: the keys of the created children.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskDecomposed {
    #[serde(serialize_with = "ser::task_keys")]
    pub children: VectorSync<TaskKey>,
    pub provider: String,
    pub model: String,
    /// The first 1000 characters of the rationale.
    pub rationale: String,
}

/// `task.synced`: a tracker sync created or changed the task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskSynced {
    pub connection: String,
    pub external_key: String,
    pub created: bool,
    /// Sorted, without duplicates.
    pub changed_fields: VectorSync<String>,
}

/// `task.commit_linked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskCommitLinked {
    pub commit_id: Id,
    pub sha: String,
    pub repository_id: Id,
    pub source: String,
}

/// `task.commit_unlinked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskCommitUnlinked {
    pub sha: String,
    /// How many links were removed.
    pub removed: i64,
}

/// `task.change_surface_computed`: totals over the task's repositories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskChangeSurfaceComputed {
    pub repositories: i64,
    pub insertions: i64,
    pub deletions: i64,
}
