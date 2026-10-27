//! The MCP output shapes of the analytics tools that wrap a list in a dict (CF-03).
//!
//! `analytics_report` answers `catalog` with `{"metrics": [...]}` and `history` with
//! `{"tasks": [...]}`; the other reports return the bare report dict (see the report DTOs).

use wg_json::{Value, dict};

/// `analytics_report` `catalog`: `{"metrics": [MetricDefinition, ...]}`.
pub fn catalog(metrics: Vec<Value>) -> Value {
    dict!({ "metrics": Value::List(metrics) })
}

/// `analytics_report` `history`: `{"tasks": [TaskHistoryRow, ...]}`.
pub fn history(tasks: Vec<Value>) -> Value {
    dict!({ "tasks": Value::List(tasks) })
}
