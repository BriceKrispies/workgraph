//! Shell: `RunService.conclude_attempt`: how every managed run concludes. It captures the
//! worktree outcome, links the attempt's commits, applies the read-only policy, then either fails
//! the attempt and its task (with escalation) or hands the task to review or verification, and
//! releases the worktrees.
//!
//! What this feature does not own is reached through [`Collaborators`]: linking commits (the git
//! feature), marking a stack item ready (the stacks) and running verification (the verify
//! feature). [`Unported`] answers each with the error that names what it waits for, so a run that
//! reaches one ends visibly instead of pretending.

use serde_json::{Map, Value as Json, json};
use wg_clispec::args::{get, s};
use wg_exec::attempt::submit_check;
use wg_exec::run::{Gate, VerifyStep, marks_ready, verify_step};
use wg_json::Value;
use wg_model::Task;
use wg_model::entity::AgentAttempt;
use wg_runtime::{App, index_written};
use wg_runtimespec::RequestContext;
use wg_store::Session;
use wg_tasks::accept::active_item;
use wg_tasks_feature::spec::claims::Reported;
use wg_types::{ErrorCode, Id, Permission, WgError};
use wg_verify::CheckTier;
use wg_verify_feature::service::run::RunInput;
use wg_wire::convention::json_value;

use crate::service::outcome::capture_outcome;
use crate::service::resolve::find_attempt;
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

/// The facts a submit checks before it writes anything.
struct SubmitFacts {
    attempt: AgentAttempt,
    task: Task,
    stacked: bool,
}

fn load_submit(session: &mut Session<'_>, reference: &str) -> Result<SubmitFacts, WgError> {
    let attempt = find_attempt(session, reference)?;
    let world = session.task_world(Some(attempt.workspace_id), &[])?;
    let task = world.tasks.task(attempt.task_id).cloned().ok_or_else(|| {
        WgError::new(
            ErrorCode::NotFound,
            format!("task {} not found", attempt.task_id),
        )
    })?;
    Ok(SubmitFacts {
        stacked: active_item(&world.stacks, task.id).is_some(),
        attempt,
        task,
    })
}

/// `RunService.submit_external`: concludes an external managed attempt from the commits the agent
/// made in its worktrees, through [`conclude_attempt`] with the agent's own report as the result.
/// The reply is the conclusion plus the task and attempt statuses after.
pub fn attempt_submit(
    app: &mut App,
    context: &RequestContext,
    collaborators: &mut dyn Collaborators,
    reference: &str,
    failed: bool,
    summary: Option<&str>,
    error: Option<&str>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskClaim)?;
    app.require_permission(context, Permission::AttemptWrite)?;
    app.require_permission(context, Permission::VerifyRun)?;
    let status = if failed { "failed" } else { "succeeded" };
    let facts = app
        .runtime
        .store
        .read(|session| load_submit(session, reference))?;
    let attempt_id = facts.attempt.id;
    submit_check(
        status,
        &attempt_id.to_string(),
        facts.attempt.status,
        facts.attempt.baseline_manifest_id.is_some(),
        &facts.task.key.to_string(),
        facts.task.status,
    )?;
    if facts.stacked {
        app.require_permission(context, Permission::StackWrite)?;
        // A stack child succeeds through the stack's gate, which the stacks feature does not
        // serve yet: refused before anything is written.
        if !failed {
            return Err(WgError::new(
                ErrorCode::WorkgraphError,
                "submitting a stack child is not ported yet (the stacks gate service)",
            ));
        }
    }
    let attempted = RequestContext {
        attempt: Some(attempt_id),
        ..context.clone()
    };
    let task_id = facts.task.id;
    let input = Concluding {
        report: AgentReport {
            status: status.to_owned(),
            summary: summary.map(str::to_owned),
            error: error.map(str::to_owned),
            exit_code: None,
            duration_ms: None,
            usage_fields: Map::new(),
        },
        record: None,
    };
    let outcome = conclude_attempt(
        app,
        &attempted,
        collaborators,
        task_id,
        attempt_id,
        &input,
        &|| false,
    )?;
    let (task_status, attempt_status) = app.runtime.store.read(|session| {
        let loaded = load_submit(session, &attempt_id.to_string())?;
        Ok((loaded.task.status, loaded.attempt.status))
    })?;
    let merged = match outcome {
        Json::Object(mut map) => {
            map.insert("task_status".to_owned(), json!(task_status.as_str()));
            map.insert("attempt_status".to_owned(), json!(attempt_status.as_str()));
            Json::Object(map)
        }
        other => other,
    };
    Ok(json_value(&merged))
}

/// The collaborators of a live process: the git feature links an attempt's commits and the verify
/// feature runs the checks; marking a stack item ready is reached through [`Unported`] until the
/// stacks feature serves it.
pub struct Live;

impl Collaborators for Live {
    fn link_commits(
        &mut self,
        app: &mut App,
        context: &RequestContext,
        attempt: Id,
    ) -> Result<(), WgError> {
        wg_git_feature::service::link_attempt_commits(app, context, attempt).map(|_| ())
    }

    fn mark_ready(
        &mut self,
        app: &mut App,
        context: &RequestContext,
        task: Id,
    ) -> Result<(), WgError> {
        Unported.mark_ready(app, context, task)
    }

    fn verify(
        &mut self,
        app: &mut App,
        context: &RequestContext,
        task: Id,
        attempt: Id,
        fast_only: bool,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<(String, String), WgError> {
        // `verification.run(ctx, str(task_id), attempt_id=..., tier=FAST if fast_only else None)`.
        let task = task.to_string();
        let attempt = attempt.to_string();
        let input = RunInput {
            workspace: None,
            task: &task,
            attempt: Some(&attempt),
            tier: fast_only.then_some(CheckTier::Fast),
        };
        let run = wg_verify_feature::service::run::run(app, context, &input, Some(should_cancel))?;
        Ok((s(get(&run, "id")), s(get(&run, "status"))))
    }
}
