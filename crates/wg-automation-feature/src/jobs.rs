//! The automation feature's job handler and maintenance steps (`automation/engine.py` as the worker
//! and the maintenance leader run it): the `automation_action` job that runs a queued run's
//! actions, the event pass and the schedule pass.
//!
//! The worker collects [`JOBS`] and [`MAINTENANCE`] through the feature's `FEATURE` value; the base
//! worker knows nothing of this crate.

use serde_json::{Value as Json, json};
use wg_exec_feature::service::agents::LiveAgents;
use wg_jobs::maintenance::Step;
use wg_runtime::{
    App, HandlerError, HandlerResult, JobContext, JobKind, MaintenanceStep, Queue, RequestContext,
};
use wg_types::{WgError, parse_id};

use crate::service;
use crate::spec::ACTION_JOB;
use crate::spec::execute::to_json;

/// The job kinds this feature handles.
pub static JOBS: &[JobKind] = &[JobKind {
    kind: ACTION_JOB,
    handler: automation_action,
}];

/// The maintenance steps this feature fills: the event pass, then the schedule pass.
pub static MAINTENANCE: &[MaintenanceStep] = &[
    MaintenanceStep {
        slot: Step::ProcessAutomationEvents,
        run: process_events,
    },
    MaintenanceStep {
        slot: Step::RunScheduledRules,
        run: run_scheduled,
    },
];

/// `automation_action`: `execute_run(payload["run_id"])`. A missing run is not retried; a failed
/// action is, with the queue's backoff.
fn automation_action(queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let run_id = ctx
        .job
        .payload
        .get("run_id")
        .and_then(Json::as_str)
        .ok_or_else(|| HandlerError::Exception("KeyError: 'run_id'".to_owned()))?;
    let id = parse_id(run_id).map_err(HandlerError::Failed)?;
    let outcome = service::execute::execute_run(&mut queue.app, &mut LiveAgents, id)
        .map_err(HandlerError::Failed)?;
    Ok(outcome.as_object().cloned())
}

/// The event pass (`AutomationService.process_events`): its stats.
fn process_events(app: &mut App, context: &RequestContext) -> Result<Json, WgError> {
    service::events_pass(app, context).map(|stats| to_json(&stats))
}

/// The schedule pass (`AutomationService.run_scheduled`): how many runs it recorded.
fn run_scheduled(app: &mut App, context: &RequestContext) -> Result<Json, WgError> {
    service::scheduled_pass(app, context).map(|created| json!(created))
}
