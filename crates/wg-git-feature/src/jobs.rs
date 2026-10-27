//! The git feature's job handler (`jobs/handlers.py`, `git_sync`): the `git_sync` job syncs one
//! repository, as `GitService.sync` does for `git sync`.
//!
//! The worker collects [`JOBS`] through the feature's `FEATURE` value; the base worker knows nothing
//! of this crate. The job is re-runnable as Python's is: an incremental sync ingests only the
//! commits the repository does not have yet, so a retried or repeated job adds nothing new.
//! The decisions are in [`crate::spec::jobs`]; this module is the glue that runs them.

use wg_runtime::{HandlerError, HandlerResult, JobContext, JobKind, Queue, system_context};

use crate::service::sync_repository;
use crate::spec::jobs::{self, MISSING_REPOSITORY};

/// The job kinds this feature handles.
pub static JOBS: &[JobKind] = &[JobKind {
    kind: "git_sync",
    handler: git_sync_job,
}];

/// `git_sync`: `GitService.sync(ctx, payload["repository_id"], mode=payload.get("mode",
/// "incremental"))`, the reply being the `SyncResult` dict.
///
/// Failures follow Python's worker: a missing `repository_id` is a bare `KeyError` (retried, kind
/// `exception`); an unknown repository is a `NotFound` (dead at once); a bad `mode` is a
/// `ValidationFailed`.
fn git_sync_job(queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let payload = &ctx.job.payload;
    let reference = jobs::repository_reference(payload)
        .ok_or_else(|| HandlerError::Exception(MISSING_REPOSITORY.to_owned()))?;
    let full = jobs::full_mode(payload.get("mode")).map_err(HandlerError::Failed)?;
    let id = jobs::repository_of(reference).map_err(HandlerError::Failed)?;
    // The job runs as the system, in the request's correlation and under its causing event.
    let mut context = system_context("git").map_err(HandlerError::Failed)?;
    if let Some(correlation) = ctx.job.correlation_id.clone() {
        context.correlation = correlation;
    }
    let context = context.caused_by(ctx.job.causation_id);
    let result =
        sync_repository(&mut queue.app, &context, id, full).map_err(HandlerError::Failed)?;
    Ok(Some(jobs::sync_result_object(&result)))
}
