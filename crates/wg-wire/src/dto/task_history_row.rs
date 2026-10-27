//! One accepted task of `GET /analytics/history` (MCP `analytics_report` `history`).
//!
//! The warehouse holds naive UTC wall-clock datetimes, so `accepted_at` prints as Python's
//! `isoformat()` of a naive datetime: no offset.

use wg_json::{LocalDateTime, Value};

use crate::convention::local_datetime;
use crate::dto::Dto;
use crate::wire_dto;

wire_dto! {
    /// The measured characteristics of one accepted task, fields in the order
    /// `AnalyticsService.task_history` selects them. Measures may be absent (`None`). It is a dict
    /// with no OpenAPI component schema.
    #[derive(Clone)]
    pub struct TaskHistoryRow<'a>("TaskHistoryRow") {
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
        #[serde(serialize_with = "local_datetime")]
        pub accepted_at: LocalDateTime,
    }
}

/// One history row.
pub fn from_parts(row: &TaskHistoryRow<'_>) -> Value {
    row.dict()
}
