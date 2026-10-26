//! The dashboard bundle of `GET /analytics/overview` (MCP `analytics_report` `overview`).

use wg_json::Value;

use crate::convention::dict;

/// The name of the bundle dict; it has no OpenAPI component schema.
pub const NAME: &str = "AnalyticsOverviewOut";

/// The sections in the order `AnalyticsService.overview` builds them. Each section is the row
/// list of one query, except `module_hotspots`, which is cut to 15 rows.
pub const FIELDS: [&str; 18] = [
    "created_by_period",
    "accepted_by_period",
    "completed_by_period",
    "open_by_status",
    "by_category",
    "execution_duration",
    "queue_wait",
    "task_elapsed",
    "first_pass_success",
    "agents",
    "agent_duration",
    "churn",
    "final_change",
    "rework",
    "check_failures_by_period",
    "verification_pass_rate",
    "repository_volume",
    "module_hotspots",
];

/// The overview bundle. `sections` holds one value per [`FIELDS`] entry, in that order.
pub fn from_parts(sections: [Value; 18]) -> Value {
    dict(FIELDS.into_iter().zip(sections).collect())
}
