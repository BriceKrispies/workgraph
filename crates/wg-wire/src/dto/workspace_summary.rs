//! `WorkspaceSummary`.

use wg_json::Value;
use wg_model::entity::workspace::Workspace;

use crate::convention::{counts, int, model};
use crate::dto::workspace;

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "WorkspaceSummary";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 8] = [
    "workspace",
    "projects",
    "repositories",
    "tasks_by_status",
    "ready_tasks",
    "running_attempts",
    "queued_jobs",
    "dead_jobs",
];

/// The aggregate counts a workspace summary reports, computed by the caller from the database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub projects: i64,
    pub repositories: i64,
    /// `(status, tasks)` pairs; `ready_tasks` is read from the `ready` entry.
    pub tasks_by_status: Vec<(String, i64)>,
    pub running_attempts: i64,
    pub queued_jobs: i64,
    pub dead_jobs: i64,
}

/// The workspace summary as the API returns it.
pub fn from_model(workspace_row: &Workspace, summary: &Counts) -> Value {
    let ready = summary
        .tasks_by_status
        .iter()
        .find(|(status, _)| status == "ready")
        .map_or(0, |(_, count)| *count);
    model(
        NAME,
        vec![
            ("workspace", workspace::from_model(workspace_row)),
            ("projects", int(summary.projects)),
            ("repositories", int(summary.repositories)),
            ("tasks_by_status", counts(&summary.tasks_by_status)),
            ("ready_tasks", int(ready)),
            ("running_attempts", int(summary.running_attempts)),
            ("queued_jobs", int(summary.queued_jobs)),
            ("dead_jobs", int(summary.dead_jobs)),
        ],
    )
}
