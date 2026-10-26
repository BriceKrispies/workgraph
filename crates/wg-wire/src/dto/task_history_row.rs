//! One accepted task of `GET /analytics/history` (MCP `analytics_report` `history`).
//!
//! The warehouse holds naive UTC wall-clock datetimes, so `accepted_at` prints as Python's
//! `isoformat()` of a naive datetime: no offset.

use wg_json::{LocalDateTime, Value};

use crate::convention::{dict, optional_int, optional_text, text};

/// The name of the row dict; it has no OpenAPI component schema.
pub const NAME: &str = "TaskHistoryRow";

/// The fields in the order `AnalyticsService.task_history` selects them.
pub const FIELDS: [&str; 18] = [
    "key",
    "title",
    "category",
    "status",
    "attempts",
    "retries",
    "execution_ms",
    "queue_wait_ms",
    "elapsed_ms",
    "commits",
    "churn_insertions",
    "churn_deletions",
    "final_insertions",
    "final_deletions",
    "files_touched",
    "repositories_touched",
    "reopen_count",
    "accepted_at",
];

/// The measured characteristics of one accepted task. Measures may be absent (`None`).
#[derive(Debug, Clone)]
pub struct TaskHistoryRow<'a> {
    pub key: &'a str,
    pub title: &'a str,
    pub category: Option<&'a str>,
    pub status: &'a str,
    pub attempts: Option<i64>,
    pub retries: Option<i64>,
    pub execution_ms: Option<i64>,
    pub queue_wait_ms: Option<i64>,
    pub elapsed_ms: Option<i64>,
    pub commits: Option<i64>,
    pub churn_insertions: Option<i64>,
    pub churn_deletions: Option<i64>,
    pub final_insertions: Option<i64>,
    pub final_deletions: Option<i64>,
    pub files_touched: Option<i64>,
    pub repositories_touched: Option<i64>,
    pub reopen_count: Option<i64>,
    /// The naive wall-clock time the task was accepted (the warehouse's `accepted_at`).
    pub accepted_at: LocalDateTime,
}

/// One history row, fields in [`FIELDS`] order.
pub fn from_parts(row: &TaskHistoryRow<'_>) -> Value {
    dict(vec![
        ("key", text(row.key)),
        ("title", text(row.title)),
        ("category", optional_text(row.category)),
        ("status", text(row.status)),
        ("attempts", optional_int(row.attempts)),
        ("retries", optional_int(row.retries)),
        ("execution_ms", optional_int(row.execution_ms)),
        ("queue_wait_ms", optional_int(row.queue_wait_ms)),
        ("elapsed_ms", optional_int(row.elapsed_ms)),
        ("commits", optional_int(row.commits)),
        ("churn_insertions", optional_int(row.churn_insertions)),
        ("churn_deletions", optional_int(row.churn_deletions)),
        ("final_insertions", optional_int(row.final_insertions)),
        ("final_deletions", optional_int(row.final_deletions)),
        ("files_touched", optional_int(row.files_touched)),
        (
            "repositories_touched",
            optional_int(row.repositories_touched),
        ),
        ("reopen_count", optional_int(row.reopen_count)),
        ("accepted_at", Value::LocalDateTime(row.accepted_at)),
    ])
}
