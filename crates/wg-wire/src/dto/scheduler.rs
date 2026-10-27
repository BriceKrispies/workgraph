//! The scheduler dict of `GET /ws/{ws}/scheduler`.

use wg_json::{Value, dict};

use crate::convention::{optional_text, text};

/// The name of the scheduler dict; it has no OpenAPI component schema.
pub const NAME: &str = "SchedulerOut";

/// The fields in the order the route builds them.
pub const FIELDS: [&str; 3] = ["enabled", "auto_schedule", "eligible"];

/// The keys of one eligible-task entry, in the order `Scheduler.eligible` builds them.
pub const ELIGIBLE_FIELDS: [&str; 6] = [
    "task_id",
    "key",
    "priority",
    "project_id",
    "agent",
    "workspace_id",
];

/// The scheduler state. `eligible` holds the entries the scheduler ranked, in that order.
pub fn from_parts(enabled: bool, auto_schedule: bool, eligible: Vec<Value>) -> Value {
    dict!({
        "enabled": Value::Bool(enabled),
        "auto_schedule": Value::Bool(auto_schedule),
        "eligible": Value::List(eligible),
    })
}

/// One eligible-task entry, from the values `Scheduler.eligible` collects.
pub fn eligible_entry(
    task_id: &str,
    key: &str,
    priority: &str,
    project_id: Option<&str>,
    agent: Option<&str>,
    workspace_id: &str,
) -> Value {
    dict!({
        "task_id": text(task_id),
        "key": text(key),
        "priority": text(priority),
        "project_id": optional_text(project_id),
        "agent": optional_text(agent),
        "workspace_id": text(workspace_id),
    })
}
