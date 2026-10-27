//! The verification job handler (`jobs/handlers.py`, `verify`): runs the checks that apply to the
//! payload's task, with the job's cancellation request ending the check in flight.

use wg_runtime::{HandlerError, HandlerResult, JobContext, JobKind, Queue, system_context};
use wg_verify::parse_tier;

use crate::service::run::{RunInput, run};
use crate::spec::run::{job_context, job_payload, job_result};

/// The job kinds this feature handles.
pub static JOBS: &[JobKind] = &[JobKind {
    kind: "verify",
    handler: verify,
}];

/// `verify`: `app.verification.run(ctx, None, payload["task_id"], attempt_id=..., tier=...)`. The
/// reply is the id and status of the run.
fn verify(queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let payload = job_payload(&ctx.job.payload).map_err(HandlerError::Exception)?;
    let tier = parse_tier(payload.tier.as_deref()).map_err(HandlerError::Failed)?;
    let context = job_context(
        system_context("verifier").map_err(HandlerError::Failed)?,
        ctx.job.correlation_id.clone(),
        ctx.job.causation_id,
    );
    let probe = ctx.clone();
    let cancel = move || probe.clone().cancel_requested();
    let input = RunInput {
        workspace: None,
        task: &payload.task_id,
        attempt: payload.attempt_id.as_deref(),
        tier,
    };
    let out = run(&mut queue.app, &context, &input, Some(&cancel)).map_err(HandlerError::Failed)?;
    Ok(Some(job_result(&out)))
}
