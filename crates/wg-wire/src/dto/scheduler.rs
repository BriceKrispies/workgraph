//! The scheduler dict of `GET /ws/{ws}/scheduler`. It has no OpenAPI component schema.

use wg_json::Value;

use crate::wire_dto;

wire_dto! {
    /// The scheduler state, fields in the order the route builds them. `eligible` holds the
    /// entries the scheduler ranked, in that order.
    pub struct SchedulerOut("SchedulerOut")
    dict from_parts(enabled: bool, auto_schedule: bool, eligible: Vec<Value>) {
        enabled: bool = enabled,
        auto_schedule: bool = auto_schedule,
        eligible: Vec<Value> = eligible,
    }
}

wire_dto! {
    /// One eligible-task entry, from the values `Scheduler.eligible` collects.
    pub struct Eligible<'a>("SchedulerEligible")
    dict eligible_entry(
        task_id: &'a str,
        key: &'a str,
        priority: &'a str,
        project_id: Option<&'a str>,
        agent: Option<&'a str>,
        workspace_id: &'a str
    ) {
        task_id: &'a str = task_id,
        key: &'a str = key,
        priority: &'a str = priority,
        project_id: Option<&'a str> = project_id,
        agent: Option<&'a str> = agent,
        workspace_id: &'a str = workspace_id,
    }
}
