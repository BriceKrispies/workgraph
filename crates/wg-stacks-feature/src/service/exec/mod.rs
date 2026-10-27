//! The stack executor (`integrations/git/stack_integration.py`, `stack_verification.py`,
//! `stack_landing.py`): the `stack_integrate` and `stack_verify` jobs, the landing of a verified
//! stack, the rebuild of integration branches, and the manual ejection.
//!
//! Every transaction is a decision of [`crate::spec::exec`]; this module orders them around the git
//! commands and the check processes, which run outside any transaction. The steps share state on
//! the application, so they are methods of [`Exec`], a borrow of the [`App`] that dereferences to
//! it (an inherent impl cannot be written for a type of another crate).
//!
//! * [`worktrees`]: the managed worktrees of integration and bisection;
//! * [`integrate`]: the merge queue of one repository;
//! * [`verification`]: the verification run;
//! * [`verify`]: verification of the tip, bisection, ejection and the rebuild;
//! * [`land`]: landing, rebasing and releasing.

/// A git runner for `path` with the default timeout.
macro_rules! git_at {
    ($path:expr) => {
        wg_git::Git {
            path: std::path::Path::new(&$path).to_path_buf(),
            timeout_ms: wg_runtimespec::effects::GIT_TIMEOUT_MS,
        }
    };
}

mod integrate;
mod land;
mod verification;
mod verify;
mod worktrees;

use serde_json::Value;
use wg_git::{Git, RunOptions};
use wg_runtime::{App, RequestContext};
use wg_stacks::merge::MergeOutcome;
use wg_store::Session;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_types::{Id, WgError};

use crate::service::load_world;
use crate::spec::git_failure;

/// The executor's view of the application: the steps of a stack job, as methods.
pub(crate) struct Exec<'a>(pub(crate) &'a mut App);

/// The task world of the stack's workspace, with the stacks' repository rows.
pub(crate) fn stack_world(session: &mut Session<'_>, stack_id: Id) -> Result<TaskWorld, WgError> {
    let stack = session
        .stack_by_id(stack_id)?
        .ok_or_else(|| WgError::not_found(format!("stack {stack_id} not found")))?;
    load_world(session, stack.workspace_id)
}

/// `merge_head`: merges `head` into the worktree's branch as the integration identity. A
/// conflicting merge is aborted, so the branch does not move.
impl Exec<'_> {
    pub(crate) fn merge_head(
        &mut self,
        git: &Git,
        head: &str,
        message: &str,
    ) -> Result<MergeOutcome, WgError> {
        let tip = git
            .head()
            .map_err(|err| git_failure(&err))?
            .ok_or_else(|| {
                WgError::git(format!(
                    "integration worktree {} has no HEAD",
                    git.path.display()
                ))
            })?;
        if git
            .is_ancestor(head, &tip)
            .map_err(|err| git_failure(&err))?
        {
            return Ok(MergeOutcome::already_integrated(&tip));
        }
        let result = git
            .merge_no_ff(message, head)
            .map_err(|err| git_failure(&err))?;
        if result.returncode == 0 {
            let merged = git
                .head()
                .map_err(|err| git_failure(&err))?
                .unwrap_or_default();
            return Ok(MergeOutcome::merged(&merged));
        }
        let unmerged = git
            .run_with(
                &["diff", "--name-only", "--diff-filter=U"],
                RunOptions::UNCHECKED,
            )
            .map_err(|err| git_failure(&err))?;
        git.merge_abort(RunOptions::UNCHECKED)
            .map_err(|err| git_failure(&err))?;
        Ok(MergeOutcome::conflicted(
            &tip,
            &unmerged.stdout,
            &result.stderr,
        ))
    }
}

/// `StackIntegrator.integrate`: merges the due items of one repository of the stack.
pub fn integrate_job(
    app: &mut App,
    context: &RequestContext,
    stack_id: Id,
    repository_id: Id,
) -> Result<Value, WgError> {
    Exec(app).stack_integrate_job(context, stack_id, repository_id)
}

/// `StackVerifier.verify`: verifies the integration tip once the stack has settled; bisects and
/// ejects on failure. `cancel` is polled at each check boundary.
pub fn verify_job(
    app: &mut App,
    context: &RequestContext,
    stack_id: Id,
    cancel: &dyn Fn() -> bool,
) -> Result<Value, WgError> {
    Exec(app).stack_verify_job(context, stack_id, cancel)
}

/// `StackVerifier.eject`: ejects a merged child an operator has identified as the stack's failure,
/// then rebuilds the integration branches without it.
pub fn eject(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
    task: &str,
    reason: &str,
) -> Result<wg_json::Value, WgError> {
    Exec(app).stack_eject(context, workspace, parent, task, reason)
}

/// `StackLander.rebuild_without`: the integration branches are rebuilt from their current bases,
/// after a release or an ejection.
pub fn rebuild_without(
    app: &mut App,
    context: &RequestContext,
    stack_id: Id,
) -> Result<(), WgError> {
    Exec(app).rebuild_without(context, stack_id)
}

/// `StackLander.land`: accepts a verified stack's parent and lands its children. Returns the
/// parent's acceptance, or `None` when the default branch moved and the stack is being rebuilt to
/// land once it verifies again. A refusal is recorded as an event and returned.
pub fn land_stack(
    app: &mut App,
    context: &RequestContext,
    stack_id: Id,
    mode: &str,
    reason: &str,
    guard: bool,
) -> Result<Option<Id>, WgError> {
    Exec(app).land_stack(context, stack_id, mode, reason, guard)
}

/// `_integration_busy`: integration work (`stack_integrate`, `stack_verify`) for this stack is
/// queued or running.
pub fn integration_busy(session: &mut Session<'_>, stack_id: Id) -> Result<bool, WgError> {
    let mut found = Vec::new();
    for kind in wg_stacks::landing::INTEGRATION_JOB_KINDS {
        for job in session.active_slice(kind)?.jobs.values() {
            found.push(wg_stacks::landing::JobRef {
                kind: job.kind.clone(),
                status: job.status.as_str().to_owned(),
                stack_id: job
                    .payload
                    .get("stack_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            });
        }
    }
    Ok(wg_stacks::landing::integration_busy(
        &found,
        &stack_id.to_string(),
    ))
}

/// `VerificationService._land_stack`: lands the stack and answers the parent's acceptance. A
/// landing that was deferred, because the default branch moved, is a conflict: the stack was
/// rebuilt on the new branch and lands once it verifies again.
pub fn land_stack_accepted(
    app: &mut App,
    context: &RequestContext,
    stack_id: Id,
    mode: &str,
    reason: &str,
    guard: bool,
) -> Result<wg_json::Value, WgError> {
    let Some(acceptance_id) = land_stack(app, context, stack_id, mode, reason, guard)? else {
        return Err(WgError::conflict(
            "the default branch moved since the stack was verified; the stack was rebuilt on the \
             new branch and lands automatically once it verifies",
        )
        .detail("stack_id", stack_id.to_string())
        .detail("landing", "deferred"));
    };
    app.runtime.query(
        |session| {
            session.fetch::<wg_model::entity::TaskAcceptance>(&wg_model::RowKey::TaskAcceptance(
                acceptance_id,
            ))
        },
        |row| {
            row.as_ref()
                .map(wg_wire::dto::verification::acceptance_from_model)
                .ok_or_else(|| WgError::not_found(format!("acceptance {acceptance_id} not found")))
        },
    )
}
