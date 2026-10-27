//! The stack job handlers (`integrations/git/stack_integration.py`, `stack_verification.py`): the
//! `stack_integrate` and `stack_verify` kinds the worker claims. Each reads its payload, takes the
//! system identity of the job and calls the executor in [`crate::service::exec`].

use serde_json::Value;
use wg_model::entity::JsonObject;
use wg_runtime::{HandlerError, HandlerResult, JobContext, JobKind, Queue, system_context};
use wg_types::{Id, WgError};

use crate::service::exec;
use crate::spec::exec::job_result;
use crate::spec::job_context;

/// The job kinds of the feature.
pub const KINDS: &[JobKind] = &[
    JobKind {
        kind: "stack_integrate",
        handler: stack_integrate,
    },
    JobKind {
        kind: "stack_verify",
        handler: stack_verify,
    },
];

/// An id member of a job payload.
fn payload_id(payload: &JsonObject, key: &str) -> Result<Id, WgError> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| WgError::workgraph(format!("the job payload has no {key}")))
        .and_then(wg_types::parse_id)
}

/// `stack_integrate`.
fn stack_integrate(queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let context = job_context(
        ctx.job.correlation_id.clone(),
        ctx.job.causation_id,
        system_context("stacks"),
    )
    .map_err(HandlerError::Failed)?;
    let ids = payload_id(&ctx.job.payload, "stack_id")
        .and_then(|stack| payload_id(&ctx.job.payload, "repository_id").map(|repo| (stack, repo)));
    let (stack_id, repository_id) = ids.map_err(HandlerError::Failed)?;
    exec::integrate_job(&mut queue.app, &context, stack_id, repository_id)
        .map(|value| job_result(&value))
        .map_err(HandlerError::Failed)
}

/// `stack_verify`.
fn stack_verify(queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let context = job_context(
        ctx.job.correlation_id.clone(),
        ctx.job.causation_id,
        system_context("stacks"),
    )
    .map_err(HandlerError::Failed)?;
    let stack_id = payload_id(&ctx.job.payload, "stack_id").map_err(HandlerError::Failed)?;
    let flag = ctx.cancel_flag();
    let cancelled = move || flag.load(std::sync::atomic::Ordering::SeqCst);
    exec::verify_job(&mut queue.app, &context, stack_id, &cancelled)
        .map(|value| job_result(&value))
        .map_err(HandlerError::Failed)
}
