//! The result of `GET /analytics/coupling` (`coupling_report`): repositories changed together.

use wg_json::Value;

use crate::convention::dict;

/// The name of the report dict; it has no OpenAPI component schema.
pub const NAME: &str = "CouplingOut";

/// The fields of the report, in the order `coupling_report` returns them.
pub const FIELDS: [&str; 9] = [
    "repositories",
    "edges",
    "declared_only",
    "matrix",
    "summary",
    "commonly_changed_pairs",
    "modules_with_cross_repo_impact",
    "coupling_candidates",
    "notes",
];

/// The fields of `summary`, in order.
pub const SUMMARY_FIELDS: [&str; 7] = [
    "tasks_with_code",
    "cross_repository_tasks",
    "average_repositories_per_task",
    "median_elapsed_ms_cross_repo",
    "median_elapsed_ms_single_repo",
    "integration_checks",
    "integration_failure_rate",
];

/// The `summary` block; the values are in [`SUMMARY_FIELDS`] order.
pub fn summary(values: [Value; 7]) -> Value {
    dict(SUMMARY_FIELDS.into_iter().zip(values).collect())
}

/// The report; the sections are in [`FIELDS`] order.
pub fn from_parts(sections: [Value; 9]) -> Value {
    dict(FIELDS.into_iter().zip(sections).collect())
}
