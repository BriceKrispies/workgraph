//! Shell: `RunService.conclude_attempt`: how every managed run concludes. It captures the
//! worktree outcome, links the attempt's commits, applies the read-only policy, then either fails
//! the attempt and its task (with escalation) or hands the task to review or verification, and
//! releases the worktrees.
//!
//! What this feature does not own is reached through [`Collaborators`]: linking commits (the git
//! feature), marking a stack item ready (the stacks) and running verification (the verify
//! feature). [`Unported`] answers each with the error that names what it waits for, so a run that
//! reaches one ends visibly instead of pretending.

use serde_json::{Value as Json, json};
use wg_exec::run::{Gate, VerifyStep, marks_ready, verify_step};
use wg_runtime::{App, index_written};
use wg_runtimespec::RequestContext;
use wg_tasks_feature::spec::claims::Reported;
use wg_types::{ErrorCode, Id, WgError};

use crate::service::outcome::capture_outcome;
use crate::service::worktrees::worktree_release;
use crate::spec::conclude::{
    AgentReport, EndFacts, RunRecord, Stage, blocked, complete_stage, conclude_first,
    escalate_task, fail_verification, succeed,
};
use crate::spec::config::execution_number;
use crate::spec::gate::gate;

/// What concluding a run needs from the features that own it.
pub trait Collaborators {
    /// `GitService.link_attempt_commits`: attributes the attempt's commits to its task.
    fn link_commits(
        &mut self,
        app: &mut App,
        context: &RequestContext,
        attempt: Id,
    ) -> Result<(), WgError>;

    /// `StackService.mark_ready`: a stack child whose fast tier passed becomes ready.
    fn mark_ready(
        &mut self,
        app: &mut App,
        context: &RequestContext,
        task: Id,
    ) -> Result<(), WgError>;

    /// `VerificationService.run`: runs the task's checks for the attempt (the fast tier only when
    /// `fast_only`); returns the id and status of the verification run.
    fn verify(
        &mut self,
        app: &mut App,
        context: &RequestContext,
        task: Id,
        attempt: Id,
        fast_only: bool,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<(String, String), WgError>;
}

/// The collaborators that are not ported yet: each answers with what it waits for.
pub struct Unported;

impl Collaborators for Unported {
    fn link_commits(&mut self, _: &mut App, _: &RequestContext, _: Id) -> Result<(), WgError> {
        Err(blocked("linking an attempt's commits (git feature)"))
    }

    fn mark_ready(&mut self, _: &mut App, _: &RequestContext, _: Id) -> Result<(), WgError> {
        Err(blocked("marking a stack item ready (stacks)"))
    }

    fn verify(
        &mut self,
        _: &mut App,
        _: &RequestContext,
        _: Id,
        _: Id,
        _: bool,
        _: &dyn Fn() -> bool,
    ) -> Result<(String, String), WgError> {
        Err(blocked("running verification (verify feature)"))
    }
}

/// The agent's report and the run's records, as a conclusion takes them.
#[derive(Debug, Clone)]
pub struct Concluding {
    pub report: AgentReport,
    pub record: Option<RunRecord>,
}

/// `conclude_attempt`. Returns the result the job reports: the status and error of a run that
/// ended, or `succeeded` with the verification and the outcome of one that stands.
#[allow(clippy::too_many_arguments)]
pub fn conclude_attempt(
    app: &mut App,
    context: &RequestContext,
    collaborators: &mut dyn Collaborators,
    task_id: Id,
    attempt_id: Id,
    input: &Concluding,
    should_cancel: &dyn Fn() -> bool,
) -> Result<Json, WgError> {
    // Without a captured outcome the attempt's commits are unknown, so it cannot count as a
    // success: a stack would treat the child as having changed nothing.
    let mut captured = match capture_outcome(app, context, attempt_id) {
        Ok(found) => found,
        Err(error) => crate::spec::conclude::Captured {
            error: Some(error.message),
            ..crate::spec::conclude::Captured::default()
        },
    };
    if captured.error.is_none()
        && let Err(error) = collaborators.link_commits(app, context, attempt_id)
    {
        captured.link_error = Some(error.message);
    }
    let max_attempts = execution_number(&app.config()?, "max_attempts_per_task", 3);
    let stage = app
        .runtime
        .decide_then(
            context,
            |session| load(session, attempt_id),
            |(attempt, world, attempts), stamp| {
                let facts = EndFacts {
                    world,
                    attempts,
                    max_attempts,
                };
                conclude_first(
                    &facts,
                    stamp,
                    attempt,
                    &input.report,
                    &captured.facts(),
                    input.record.as_ref(),
                )
            },
            index_written,
        )?
        .reply;
    if stage == Stage::Ended {
        worktree_release(app, context, attempt_id, false, false)?;
        return Ok(json!({"status": input.report.status, "error": input.report.error}));
    }

    // The agent claims success: that is not verification.
    let waits = app
        .runtime
        .decide_then(
            context,
            |session| load_reported(session, attempt_id),
            |reported, stamp| {
                let Some(attempt) = reported.attempt.as_ref() else {
                    return Err(WgError::new(
                        ErrorCode::NotFound,
                        format!("attempt {attempt_id} not found"),
                    ));
                };
                complete_stage(
                    reported,
                    attempt,
                    task_id,
                    input.report.summary.as_deref().unwrap_or(""),
                    input.report.exit_code,
                    input.report.duration_ms,
                    stamp,
                )
            },
            index_written,
        )?
        .reply;
    let verification = if waits {
        Some(verify_attempt(
            app,
            context,
            collaborators,
            task_id,
            attempt_id,
            input,
            should_cancel,
            max_attempts,
        )?)
    } else {
        None
    };
    worktree_release(app, context, attempt_id, false, false)?;
    let mut outcome = captured.report();
    if let Some(found) = outcome.as_object_mut() {
        found.remove("commits");
    }
    Ok(json!({"status": "succeeded", "verification": verification, "outcome": outcome}))
}

type EndLoad = (
    wg_model::entity::AgentAttempt,
    wg_storecodec::task_world_sql::TaskWorld,
    Vec<wg_model::entity::AgentAttempt>,
);

fn load(session: &mut wg_store::Session<'_>, attempt_id: Id) -> Result<EndLoad, WgError> {
    let attempt = session.attempt_by_id(attempt_id)?.ok_or_else(|| {
        WgError::new(
            ErrorCode::NotFound,
            format!("attempt {attempt_id} not found"),
        )
    })?;
    let world = session.task_world(Some(attempt.workspace_id), &[])?;
    let attempts = session.attempts_of_task(attempt.task_id, &[])?;
    Ok((attempt, world, attempts))
}

fn load_reported(session: &mut wg_store::Session<'_>, attempt_id: Id) -> Result<Reported, WgError> {
    let attempt = session.attempt_by_id(attempt_id)?;
    let workspace = attempt.as_ref().map(|row| row.workspace_id);
    let world = session.task_world(workspace, &[])?;
    let configs = match workspace {
        Some(id) => session.verification_configs(id)?,
        None => Vec::new(),
    };
    Ok(Reported {
        world,
        configs,
        attempt,
    })
}

/// The gate and the verification of a task that waits for it. Returns the verification report.
#[allow(clippy::too_many_arguments)]
fn verify_attempt(
    app: &mut App,
    context: &RequestContext,
    collaborators: &mut dyn Collaborators,
    task_id: Id,
    attempt_id: Id,
    input: &Concluding,
    should_cancel: &dyn Fn() -> bool,
    max_attempts: i64,
) -> Result<Json, WgError> {
    // A stack child is gated by its fast tier; the full tier runs once on the integration tip.
    let reported = app
        .runtime
        .store
        .read(|session| load_reported(session, attempt_id))?;
    let gated = gate(
        &reported.world.tasks,
        &reported.world.stacks,
        &reported.configs,
        task_id,
    )?;
    let kind = gated.unwrap_or(Gate::NoStack);
    let report = match verify_step(kind) {
        VerifyStep::SkipMarkReady => {
            app.runtime.decide_then(
                context,
                |session| load(session, attempt_id).map(|(attempt, ..)| attempt),
                |attempt, stamp| {
                    succeed(
                        attempt,
                        input.report.exit_code,
                        input.report.duration_ms,
                        stamp,
                    )
                },
                index_written,
            )?;
            collaborators.mark_ready(app, context, task_id)?;
            json!({"gate": "no_fast_checks"})
        }
        VerifyStep::Run { fast_only } => {
            let ran = collaborators
                .verify(app, context, task_id, attempt_id, fast_only, should_cancel)
                .and_then(|(run_id, status)| {
                    if marks_ready(kind, &status) {
                        collaborators.mark_ready(app, context, task_id)?;
                    }
                    Ok((run_id, status))
                });
            let report = match ran {
                Ok((run_id, status)) => json!({"run_id": run_id, "status": status}),
                Err(error) => {
                    app.runtime.decide_then(
                        context,
                        |session| load(session, attempt_id).map(|(attempt, ..)| attempt),
                        |attempt, stamp| fail_verification(attempt, &error.message, stamp),
                        index_written,
                    )?;
                    json!({"error": error.message})
                }
            };
            app.runtime.decide_then(
                context,
                |session| load(session, attempt_id),
                |(attempt, world, attempts), stamp| {
                    let facts = EndFacts {
                        world,
                        attempts,
                        max_attempts,
                    };
                    Ok(escalate_task(&facts, stamp, attempt.task_id))
                },
                index_written,
            )?;
            report
        }
    };
    Ok(report)
}
