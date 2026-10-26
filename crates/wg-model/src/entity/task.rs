//! The `task` family: `tasks` and the tables whose rows belong to one task: `task_files`,
//! `task_tags`, `task_links`, `task_comments`, `task_dependencies`, `task_relations`,
//! `task_projects`, `task_repositories` and `external_items`.

use rpds::VectorSync;
use wg_types::{CommentKind, Id, RelationType, TaskKey, TaskPriority, TaskStatus, Timestamp};

use crate::entity::JsonObject;
use crate::write::{Row, RowKey};

/// A row of `tasks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: Id,
    pub workspace_id: Id,
    pub number: i64,
    pub key: TaskKey,
    pub title: String,
    pub description: String,
    /// SHA-256 of the normalised title and description (`content_hash`).
    pub description_hash: String,
    /// JSON list column.
    pub acceptance_criteria: VectorSync<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub category: Option<String>,
    pub parent_id: Option<Id>,
    pub assignee: Option<String>,
    pub assigned_agent: Option<String>,
    pub claimed_by: Option<String>,
    pub claim_expires_at: Option<Timestamp>,
    pub auto_execute: bool,
    pub estimated_effort: Option<JsonObject>,
    /// The `metadata` column (`metadata_` in the Python ORM).
    pub metadata: JsonObject,
    pub due_at: Option<Timestamp>,
    pub scheduled_at: Option<Timestamp>,
    /// An actor string, or `system` (not always `<kind>:<name>`).
    pub created_by: String,
    /// Optimistic-concurrency counter: every change to the row increments it.
    pub version: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    pub accepted_at: Option<Timestamp>,
    pub archived_at: Option<Timestamp>,
    pub current_classification_id: Option<Id>,
}

/// A row of `task_files`: a file the task expects to change. Primary key `(task_id, position)`;
/// `position` keeps the order the caller gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFile {
    pub task_id: Id,
    pub position: i64,
    pub path: String,
    pub anchor: Option<String>,
    pub note: String,
}

/// A row of `task_tags`. Primary key `(task_id, tag)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskTag {
    pub task_id: Id,
    pub tag: String,
}

/// A row of `task_links`: a typed link to something outside Workgraph (a pull request first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLink {
    pub id: Id,
    pub task_id: Id,
    pub kind: String,
    pub provider: String,
    pub url: String,
    pub normalized_url: String,
    /// `org/repo` for a GitHub pull request; empty for other kinds.
    pub repo_full_name: String,
    pub number: Option<i64>,
    pub repository_id: Option<Id>,
    /// Which extractors found the link (JSON list column).
    pub sources: VectorSync<String>,
    pub title: String,
    pub created_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A row of `task_comments`: a comment, progress report or result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskComment {
    pub id: Id,
    pub task_id: Id,
    pub attempt_id: Option<Id>,
    pub author: String,
    pub kind: CommentKind,
    pub body: String,
    pub data: JsonObject,
    pub created_at: Timestamp,
}

/// A row of `task_dependencies`: `task_id` cannot proceed until `depends_on_id` is accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDependency {
    pub id: Id,
    pub task_id: Id,
    pub depends_on_id: Id,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// A row of `task_relations`: a typed, non-blocking relation between two tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRelation {
    pub id: Id,
    pub source_task_id: Id,
    pub target_task_id: Id,
    pub relation_type: RelationType,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// A row of `task_projects`. Primary key `(task_id, project_id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProject {
    pub task_id: Id,
    pub project_id: Id,
}

/// A row of `task_repositories`. Primary key `(task_id, repository_id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRepository {
    pub task_id: Id,
    pub repository_id: Id,
    /// `read_write` unless the caller or the repository's access mode says otherwise.
    pub access: String,
    pub created_at: Timestamp,
}

/// A row of `task_decisions`: a question the task waits on and its answer. `options` is a JSON
/// list column; a decided question records one option id, or free text in `other_text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDecision {
    pub id: Id,
    pub workspace_id: Id,
    pub task_id: Id,
    pub question: String,
    pub options: Vec<DecisionOption>,
    pub status: String,
    pub chosen_option_id: Option<String>,
    pub other_text: Option<String>,
    pub note: Option<String>,
    pub decided_by: Option<String>,
    pub decided_at: Option<Timestamp>,
    pub created_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// Optimistic-concurrency counter: every change to the row increments it.
    pub version: i64,
}

/// One entry of a decision's `options` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionOption {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub recommended: bool,
}

/// A row of `external_items`: the tracker issue a task mirrors (at most one per task). The
/// tracker owns these fields; only the sync writes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalItem {
    pub id: Id,
    pub workspace_id: Id,
    pub task_id: Id,
    /// Name of the `[trackers.<name>]` connection in configuration.
    pub connection: String,
    /// The tracker's own stable id, unchanged when the key changes.
    pub external_id: String,
    pub external_key: String,
    pub url: String,
    pub tracker_status: String,
    /// Jira's statusCategory key: `new`, `indeterminate` or `done`.
    pub tracker_status_category: String,
    pub tracker_resolution: Option<String>,
    pub remote_updated_at: Option<Timestamp>,
    pub snapshot: JsonObject,
    pub snapshot_hash: String,
    pub synced_at: Option<Timestamp>,
    /// Set when a full sync no longer finds the issue; cleared when it reappears.
    pub missing_since: Option<Timestamp>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Task {
    pub fn row_key(&self) -> RowKey {
        RowKey::Task(self.id)
    }
}

impl TaskFile {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskFile {
            task_id: self.task_id,
            position: self.position,
        }
    }
}

impl TaskTag {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskTag {
            task_id: self.task_id,
            tag: self.tag.clone(),
        }
    }
}

impl TaskLink {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskLink(self.id)
    }
}

impl TaskComment {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskComment(self.id)
    }
}

impl TaskDependency {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskDependency(self.id)
    }
}

impl TaskRelation {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskRelation(self.id)
    }
}

impl TaskProject {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskProject {
            task_id: self.task_id,
            project_id: self.project_id,
        }
    }
}

impl TaskRepository {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskRepository {
            task_id: self.task_id,
            repository_id: self.repository_id,
        }
    }
}

impl TaskDecision {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskDecision(self.id)
    }
}

impl ExternalItem {
    pub fn row_key(&self) -> RowKey {
        RowKey::ExternalItem(self.id)
    }
}

impl From<Task> for Row {
    fn from(row: Task) -> Self {
        Row::Task(row)
    }
}

impl From<TaskFile> for Row {
    fn from(row: TaskFile) -> Self {
        Row::TaskFile(row)
    }
}

impl From<TaskTag> for Row {
    fn from(row: TaskTag) -> Self {
        Row::TaskTag(row)
    }
}

impl From<TaskLink> for Row {
    fn from(row: TaskLink) -> Self {
        Row::TaskLink(row)
    }
}

impl From<TaskComment> for Row {
    fn from(row: TaskComment) -> Self {
        Row::TaskComment(row)
    }
}

impl From<TaskDependency> for Row {
    fn from(row: TaskDependency) -> Self {
        Row::TaskDependency(row)
    }
}

impl From<TaskRelation> for Row {
    fn from(row: TaskRelation) -> Self {
        Row::TaskRelation(row)
    }
}

impl From<TaskProject> for Row {
    fn from(row: TaskProject) -> Self {
        Row::TaskProject(row)
    }
}

impl From<TaskRepository> for Row {
    fn from(row: TaskRepository) -> Self {
        Row::TaskRepository(row)
    }
}

impl From<TaskDecision> for Row {
    fn from(row: TaskDecision) -> Self {
        Row::TaskDecision(row)
    }
}

impl From<ExternalItem> for Row {
    fn from(row: ExternalItem) -> Self {
        Row::ExternalItem(row)
    }
}
