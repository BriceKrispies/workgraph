//! The MCP output shapes of the analytics tools that wrap a list in a dict (CF-03).
//!
//! `analytics_report` answers `catalog` with `{"metrics": [...]}` and `history` with
//! `{"tasks": [...]}`; the other reports return the bare report dict (see the report DTOs).

use wg_json::Value;

use crate::wire_dto;

wire_dto! {
    /// `analytics_report` `catalog`: `{"metrics": [MetricDefinition, ...]}`.
    pub struct Catalog("Catalog") dict catalog(metrics: Vec<Value>) {
        metrics: Vec<Value> = metrics,
    }
}

wire_dto! {
    /// `analytics_report` `history`: `{"tasks": [TaskHistoryRow, ...]}`.
    pub struct History("History") dict history(tasks: Vec<Value>) {
        tasks: Vec<Value> = tasks,
    }
}
