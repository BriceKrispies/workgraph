//! The git feature's job handlers (`jobs/handlers.py`): `git_sync` syncs one repository, as
//! `GitService.sync` does for `git sync`; `compute_change_surface` measures a task's change surface,
//! as `GitService.compute_change_surface` does for `git surface`.
//!
//! The worker collects [`JOBS`] through the feature's `FEATURE` value; the base worker knows nothing
//! of this crate. The job is re-runnable as Python's is: an incremental sync ingests only the
//! commits the repository does not have yet, so a retried or repeated job adds nothing new.
//! The decisions are in [`crate::spec::jobs`]; this module is the glue that runs them.

use wg_runtime::spec::RequestContext;
use wg_runtime::{HandlerError, HandlerResult, JobContext, JobKind, Queue, system_context};
use wg_types::WgError;

use crate::service::{analyze_commit, git_surface, sync_repository};
use crate::spec::jobs::{self, MISSING_COMMIT_IDS, MISSING_REPOSITORY, MISSING_TASK};

/// The job kinds this feature handles.
pub static JOBS: &[JobKind] = &[
    JobKind {
        kind: "git_sync",
        handler: git_sync_job,
    },
    JobKind {
        kind: "compute_change_surface",
        handler: compute_change_surface_job,
    },
    JobKind {
        kind: "analyze_commits",
        handler: analyze_commits_job,
    },
];

/// The request context a job runs in: the system's, in the job's correlation and under its causing
/// event, as the Python worker's `_ctx` builds it.
fn job_context(ctx: &JobContext, component: &str) -> Result<RequestContext, WgError> {
    let mut context = system_context(component)?;
    if let Some(correlation) = ctx.job.correlation_id.clone() {
        context.correlation = correlation;
    }
    Ok(context.caused_by(ctx.job.causation_id))
}

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
    let context = job_context(ctx, "git").map_err(HandlerError::Failed)?;
    let result =
        sync_repository(&mut queue.app, &context, id, full).map_err(HandlerError::Failed)?;
    Ok(Some(jobs::sync_result_object(&result)))
}

/// `compute_change_surface`: `GitService.compute_change_surface(ctx, payload["task_id"])`, the
/// reply being `{"repositories": len(surfaces)}`.
///
/// A missing `task_id` is a bare `KeyError` (retried, kind `exception`); the task's own failures
/// (an unknown task, a git error) are the surface service's errors.
fn compute_change_surface_job(queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let reference = jobs::task_reference(&ctx.job.payload)
        .ok_or_else(|| HandlerError::Exception(MISSING_TASK.to_owned()))?;
    let context = job_context(ctx, "git").map_err(HandlerError::Failed)?;
    let replies =
        git_surface(&mut queue.app, &context, None, reference).map_err(HandlerError::Failed)?;
    Ok(Some(jobs::surface_result_object(jobs::surface_count(
        &replies,
    ))))
}

/// `analyze_commits`: `GitService.analyze_commit` for each of the payload's `commit_ids`, checking
/// for cancellation before each; the reply is `{"analyzed": n}`, n counting the analyses written.
fn analyze_commits_job(queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let ids = jobs::commit_ids(&ctx.job.payload)
        .ok_or_else(|| HandlerError::Exception(MISSING_COMMIT_IDS.to_owned()))?;
    let context = job_context(ctx, "git").map_err(HandlerError::Failed)?;
    let mut analyzed = 0;
    for id in ids {
        ctx.check_cancelled()?;
        if analyze_commit(&mut queue.app, &context, id).map_err(HandlerError::Failed)? {
            analyzed += 1;
        }
    }
    Ok(Some(jobs::analyze_result_object(analyzed)))
}
