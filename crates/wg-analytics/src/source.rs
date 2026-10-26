//! The source rows of the analytics warehouse: the columns of `analytics/warehouse.py` `TABLES`
//! that the fact tables read, and the `events` rows of the status and reopen events.
//!
//! Every column is nullable, as in the database, so each field is an `Option`. A table is a
//! persistent vector in the order a table scan returns its rows, because the first row of a tie
//! decides some fact values (`arg_min`, the commit a duplicate patch keeps).

use rpds::VectorSync;
use wg_types::Timestamp;

/// A row of `tasks`, every column the warehouse copies (`task_facts` passes them through).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TaskRow {
    pub id: Option<String>,
    pub workspace_id: Option<String>,
    pub number: Option<i64>,
    pub key: Option<String>,
    pub title: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub category: Option<String>,
    pub parent_id: Option<String>,
    pub assignee: Option<String>,
    pub assigned_agent: Option<String>,
    pub created_at: Option<Timestamp>,
    pub updated_at: Option<Timestamp>,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    pub accepted_at: Option<Timestamp>,
    pub archived_at: Option<Timestamp>,
    pub auto_execute: Option<bool>,
}

/// A row of `agent_attempts`, every column the warehouse copies.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AttemptRow {
    pub id: Option<String>,
    pub workspace_id: Option<String>,
    pub task_id: Option<String>,
    pub number: Option<i64>,
    pub status: Option<String>,
    pub provider: Option<String>,
    pub adapter: Option<String>,
    pub model: Option<String>,
    pub parent_attempt_id: Option<String>,
    pub retry_of_attempt_id: Option<String>,
    pub rerun_of_attempt_id: Option<String>,
    pub created_at: Option<Timestamp>,
    pub queued_at: Option<Timestamp>,
    pub started_at: Option<Timestamp>,
    pub finished_at: Option<Timestamp>,
    pub queue_wait_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub exit_reason: Option<String>,
    /// The JSON text of the usage object.
    pub usage: Option<String>,
    pub usage_source: Option<String>,
    pub cost_usd: Option<f64>,
    pub cost_source: Option<String>,
    pub modified_repository_ids: Option<String>,
}

/// A row of `commits`, every column the warehouse copies.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommitRow {
    pub id: Option<String>,
    pub repository_id: Option<String>,
    pub sha: Option<String>,
    pub author_name: Option<String>,
    pub author_email: Option<String>,
    pub committed_at: Option<Timestamp>,
    pub subject: Option<String>,
    pub is_merge: Option<bool>,
    pub patch_id: Option<String>,
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
    pub files_changed: Option<i64>,
    pub reachable: Option<bool>,
}

/// A row of `commit_task_links`, the columns the fact tables read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CommitLink {
    pub commit_id: Option<String>,
    pub task_id: Option<String>,
    pub confidence: Option<f64>,
}

/// A row of `commit_file_changes`, the columns the fact tables read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FileChange {
    pub commit_id: Option<String>,
    pub path: Option<String>,
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
    pub module: Option<String>,
    pub file_class: Option<String>,
}

/// A row of `commit_analyses`, the columns the fact tables read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Analysis {
    pub commit_id: Option<String>,
    pub functions_affected: Option<i64>,
    pub types_affected: Option<i64>,
}

/// A row of `task_change_surfaces`, the columns the fact tables read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Surface {
    pub task_id: Option<String>,
    pub files_changed: Option<i64>,
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
    pub renames: Option<i64>,
    pub binary_files: Option<i64>,
    pub functions_affected: Option<i64>,
    pub types_affected: Option<i64>,
}

/// A row of `task_projects`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TaskProject {
    pub task_id: Option<String>,
    pub project_id: Option<String>,
}

/// A row of `task_repositories`. The fact tables read only its task; the metrics also read the
/// repository a task is linked to.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TaskRepository {
    pub task_id: Option<String>,
    pub repository_id: Option<String>,
}

/// A row of `projects`, the columns the metric labels and filters read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProjectRow {
    pub id: Option<String>,
    pub workspace_id: Option<String>,
    pub slug: Option<String>,
}

/// A row of `repositories`, the columns the metric labels and filters read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RepositoryRow {
    pub id: Option<String>,
    pub workspace_id: Option<String>,
    pub name: Option<String>,
}

/// A row of `verification_runs`, the columns the fact tables and the metrics read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VerificationRun {
    pub id: Option<String>,
    pub workspace_id: Option<String>,
    pub task_id: Option<String>,
    pub attempt_id: Option<String>,
    pub status: Option<String>,
    pub created_at: Option<Timestamp>,
}

/// A row of `verification_check_results`, the columns the metrics read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CheckResult {
    pub run_id: Option<String>,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub repository_id: Option<String>,
    pub status: Option<String>,
    pub created_at: Option<Timestamp>,
}

/// A row of `task_relations`, the columns the metrics read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TaskRelation {
    pub source_task_id: Option<String>,
    pub relation_type: Option<String>,
    pub created_at: Option<Timestamp>,
}

/// A row of `subagent_runs`, the columns the subagent and agent-time reports read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SubagentRunRow {
    pub id: Option<String>,
    pub task_id: Option<String>,
    pub agent_type: Option<String>,
    pub model: Option<String>,
    pub requested_model: Option<String>,
    pub started_at: Option<Timestamp>,
    pub stopped_at: Option<Timestamp>,
    /// The JSON text of the usage object.
    pub usage: Option<String>,
    /// The JSON text of the telemetry object (`trial_ids`, `turns`, `tool_errors`, `time`, ...).
    pub telemetry: Option<String>,
}

/// A row of `task_acceptances`: one acceptance decision of a task.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AcceptanceRow {
    pub task_id: Option<String>,
    pub decision: Option<String>,
}

/// A row of `events` among the types the warehouse loads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Event {
    pub event_type: Option<String>,
    pub entity_id: Option<String>,
    pub occurred_at: Option<Timestamp>,
    /// The JSON text of the payload.
    pub data: Option<String>,
}

/// Every source table the fact tables read. The world root of the `Core` rules: the fields hold
/// persistent vectors only.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnalyticsInput {
    pub tasks: VectorSync<TaskRow>,
    pub attempts: VectorSync<AttemptRow>,
    pub commits: VectorSync<CommitRow>,
    pub links: VectorSync<CommitLink>,
    pub file_changes: VectorSync<FileChange>,
    pub analyses: VectorSync<Analysis>,
    pub surfaces: VectorSync<Surface>,
    pub task_projects: VectorSync<TaskProject>,
    pub task_repositories: VectorSync<TaskRepository>,
    pub verification_runs: VectorSync<VerificationRun>,
    pub check_results: VectorSync<CheckResult>,
    pub relations: VectorSync<TaskRelation>,
    pub projects: VectorSync<ProjectRow>,
    pub repositories: VectorSync<RepositoryRow>,
    pub events: VectorSync<Event>,
    pub subagent_runs: VectorSync<SubagentRunRow>,
    pub acceptances: VectorSync<AcceptanceRow>,
}
