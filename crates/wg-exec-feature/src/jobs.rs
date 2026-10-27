//! The exec feature's job handlers and maintenance step (`jobs/handlers.py` and
//! `jobs/maintenance.py` as the worker and the maintenance leader run them): the `agent_run` job
//! that runs a queued attempt, the `decompose_task` job, and the scheduler pass.
//!
//! The worker collects [`JOBS`] and [`MAINTENANCE`] through the feature's `FEATURE` value.

use serde_json::Value as Json;
use wg_jobs::maintenance::Step;
use wg_runtime::{
    App, HandlerError, HandlerResult, JobContext, JobKind, MaintenanceStep, Queue, RequestContext,
    system_context,
};
use wg_types::{WgError, parse_id};

use crate::service::agents::LiveAgents;
use crate::service::conclude::Live;
use crate::service::execute::execute;
use crate::service::scheduler::{decompose, tick};

/// The job kinds this feature handles.
pub static JOBS: &[JobKind] = &[
    JobKind {
        kind: "agent_run",
        handler: agent_run,
    },
    JobKind {
        kind: "decompose_task",
        handler: decompose_task,
    },
];

/// The maintenance step this feature fills: the scheduler pass.
pub static MAINTENANCE: &[MaintenanceStep] = &[MaintenanceStep {
    slot: Step::ScheduledRuns,
    run: scheduled_runs,
}];

/// `agent_run`: `RunService.execute(job)`.
fn agent_run(queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let result = execute(&mut queue.app, &mut LiveAgents, &mut Live, ctx)
        .map_err(HandlerError::Failed)?;
    Ok(result.as_object().cloned())
}

/// `decompose_task`: `Scheduler.decompose(task_id, create=payload.get("create", True))`.
fn decompose_task(queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let payload = &ctx.job.payload;
    let task = payload
        .get("task_id")
        .and_then(Json::as_str)
        .ok_or_else(|| HandlerError::Exception("KeyError: 'task_id'".to_owned()))
        .and_then(|text| parse_id(text).map_err(HandlerError::Failed))?;
    let create = payload.get("create").is_none_or(|flag| {
        // Python truthiness of the payload member.
        match flag {
            Json::Null => false,
            Json::Bool(value) => *value,
            Json::String(text) => !text.is_empty(),
            Json::Array(items) => !items.is_empty(),
            Json::Object(fields) => !fields.is_empty(),
            Json::Number(number) => number.as_f64().is_none_or(|n| n != 0.0),
        }
    });
    let context = system_context("scheduler").map_err(HandlerError::Failed)?;
    let report = decompose(&mut queue.app, &context, task, create, None, None)
        .map_err(HandlerError::Failed)?;
    Ok(report.as_object().cloned())
}

/// The scheduler pass (`Scheduler.tick`), which the maintenance plan runs only when execution and
/// auto scheduling are on.
fn scheduled_runs(app: &mut App, context: &RequestContext) -> Result<Json, WgError> {
    tick(app, context, &mut LiveAgents)
}
