//! `ProjectSummary`.

use wg_json::Value;
use wg_model::entity::repository::Repository;

use crate::convention::{counts, int, model};
use crate::dto::repository_brief;

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "ProjectSummary";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 6] = [
    "project",
    "repositories",
    "tasks_by_status",
    "open_tasks",
    "accepted_tasks",
    "recent_tasks",
];

/// The project summary as the API returns it.
///
/// `project` is the `ProjectOut` value from [`crate::dto::project::from_model`]. `recent_tasks` is
/// the list of `TaskBrief` values that the task DTOs (LIB-13) produce; this DTO does not build
/// them. `open_tasks` counts every task that is neither accepted nor cancelled, as the Python
/// service does.
pub fn from_model(
    project: Value,
    repositories: &[Repository],
    tasks_by_status: &[(String, i64)],
    recent_tasks: Vec<Value>,
) -> Value {
    let status_count = |key: &str| {
        tasks_by_status
            .iter()
            .find(|(status, _)| status == key)
            .map_or(0, |(_, count)| *count)
    };
    let total: i64 = tasks_by_status.iter().map(|(_, count)| *count).sum();
    let accepted = status_count("accepted");
    let closed = accepted + status_count("cancelled");
    model(
        NAME,
        vec![
            ("project", project),
            (
                "repositories",
                Value::List(
                    repositories
                        .iter()
                        .map(repository_brief::from_model)
                        .collect(),
                ),
            ),
            ("tasks_by_status", counts(tasks_by_status)),
            ("open_tasks", int(total - closed)),
            ("accepted_tasks", int(accepted)),
            ("recent_tasks", Value::List(recent_tasks)),
        ],
    )
}
