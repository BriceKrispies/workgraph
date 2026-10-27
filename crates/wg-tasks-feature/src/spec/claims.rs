//! The claim lifecycle of a task: claim, release, complete and report a result
//! (`TaskService.claim`, `release`, `complete` and `report_result`), as decisions over the task
//! world.
//!
//! Each function checks the permission, resolves the task, asks the `wg-tasks` plans what to do,
//! applies the status changes through [`transition`] and returns a [`Decision`] whose reply is
//! the `TaskOut` the route prints. Completion and result reports also read the verification
//! configurations (whether checks apply decides the next status) and the attempt a report names.

use serde_json::{Map, json};
use wg_exec::attempt::{FinishPlan, duration_ms, finish_plan, sanitize_usage};
use wg_json::Value;
use wg_model::entity::{AgentAttempt, Real, VerificationConfig};
use wg_model::event::{
    AttemptFinished, EventDraft, EventPayload, TaskClaimed, TaskCompleted, TaskReleased,
};
use wg_model::stamp::{IdSeq, Stamp, py_repr_str};
use wg_model::{Decision, TaskSlice, Write};
use wg_registry::resolve;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::accept::comment_step;
use wg_tasks::claims::{
    Caller, ClaimPlan, REASON_ATTEMPT_FAILED, REASON_COMPLETION, REASON_EXPIRED_CLAIM_RELEASED,
    plan_claim, plan_complete, plan_release, plan_report_result,
};
use wg_tasks::comments::check_attempt_owner;
use wg_tasks::deps::unsatisfied_dependencies;
use wg_tasks::transition::{Applied, Change, transition};
use wg_types::{AttemptStatus, CommentKind, ErrorCode, Id, Permission, TaskStatus, WgError};
use wg_verify::{TaskScope, applicable_checks};

use super::bodies::{ClaimRead, CompleteRead, ResultRead};
use wg_tasks::shape::task_out;

/// The attempt id a report names, and the attempt row it found (`None` when there is no such
/// attempt).
#[derive(Debug, Clone, Copy)]
pub struct AttemptRef<'a> {
    pub given: &'a str,
    pub found: Option<&'a AgentAttempt>,
}

/// What a report (complete, result, comment, progress) reads besides the task world: the
/// verification configurations of the task's workspace and the attempt it names.
#[derive(Debug, Clone)]
pub struct Reported {
    pub world: TaskWorld,
    pub configs: Vec<VerificationConfig>,
    pub attempt: Option<AgentAttempt>,
}

/// `resolve.attempt`: the attempt, or `not_found`.
pub fn attempt_of<'a>(
    given: &str,
    found: Option<&'a AgentAttempt>,
) -> Result<&'a AgentAttempt, WgError> {
    found.ok_or_else(|| {
        WgError::new(
            ErrorCode::NotFound,
            format!("attempt {} not found", py_repr_str(given)),
        )
        .with_detail("ref", json!(given))
    })
}

/// The attempt a comment names, which must belong to `task` (`_comment`).
pub fn owned_attempt(
    task: Id,
    given: Option<&str>,
    found: Option<&AgentAttempt>,
) -> Result<Option<Id>, WgError> {
    given
        .map(|text| {
            let attempt = attempt_of(text, found)?;
            check_attempt_owner(task, attempt.task_id)?;
            Ok(attempt.id)
        })
        .transpose()
}

fn caller_of<'a>(stamp: &Stamp, actor: &'a str) -> Caller<'a> {
    Caller {
        actor,
        is_admin: stamp.has(Permission::TaskAdmin),
    }
}

fn gone(task: Id) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("task {task} vanished from the slice"),
    )
}

/// `TaskService.claim`.
pub fn claim_task(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    read: &ClaimRead,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskClaim])?;
    let tasks = &world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let blockers = unsatisfied_dependencies(tasks, &world.stacks, task.id);
    let actor = stamp.actor.to_string();
    let plan = plan_claim(
        task,
        &actor,
        read.agent.as_deref(),
        read.lease_minutes,
        read.expected_version,
        stamp.at,
        &blockers,
    )?;
    match plan {
        ClaimPlan::Renew { expires_at } => {
            let next = wg_model::Task {
                claim_expires_at: Some(expires_at),
                version: task.version + 1,
                updated_at: stamp.at,
                ..task.clone()
            };
            let decision = Decision::new(()).write(Write::update(next));
            let state = tasks.apply_all(decision.writes.iter());
            let after = state.task(task.id).ok_or_else(|| gone(task.id))?;
            let out = task_out(&state, after, None);
            Ok(decision.with_reply(out))
        }
        ClaimPlan::Take {
            claimant,
            expires_at,
            release_expired,
            set_assignee,
        } => {
            let start = Applied {
                decision: Decision::new(()),
                ids: stamp.ids(),
                tasks: tasks.clone(),
            };
            let released = if release_expired.is_some() {
                step(
                    start,
                    world,
                    stamp,
                    task.id,
                    TaskStatus::Ready,
                    Change {
                        reason: Some(REASON_EXPIRED_CLAIM_RELEASED),
                        ..Change::default()
                    },
                )?
            } else {
                start
            };
            let (status_id, _) = released.ids.next();
            let reason = format!("claimed by {claimant}");
            let moved = step(
                released,
                world,
                stamp,
                task.id,
                TaskStatus::Claimed,
                Change {
                    reason: Some(&reason),
                    ..Change::default()
                },
            )?;
            let current = moved.tasks.task(task.id).ok_or_else(|| gone(task.id))?;
            let held = wg_model::Task {
                claimed_by: Some(claimant.clone()),
                claim_expires_at: Some(expires_at),
                assignee: if set_assignee {
                    Some(claimant.clone())
                } else {
                    current.assignee.clone()
                },
                ..current.clone()
            };
            let (event_id, _) = moved.ids.next();
            let claimed = EventDraft::new(
                stamp,
                event_id,
                Some(task.workspace_id),
                task.id,
                EventPayload::TaskClaimed(TaskClaimed {
                    claimed_by: claimant,
                    expires_at,
                }),
            )
            .caused_by(status_id);
            let decision = moved.decision.write(Write::update(held)).event(claimed);
            let state = tasks.apply_all(decision.writes.iter());
            let after = state.task(task.id).ok_or_else(|| gone(task.id))?;
            let out = task_out(&state, after, None);
            Ok(decision.with_reply(out))
        }
    }
}

/// One status change chained onto `start`: its writes and events join `start`'s.
fn step(
    start: Applied,
    world: &TaskWorld,
    stamp: &Stamp,
    task: Id,
    target: TaskStatus,
    change: Change<'_>,
) -> Result<Applied, WgError> {
    let next = transition(
        &start.tasks,
        &world.stacks,
        stamp,
        start.ids,
        task,
        target,
        change,
    )?;
    Ok(Applied {
        decision: start.decision.then(next.decision),
        ids: next.ids,
        tasks: next.tasks,
    })
}

/// `TaskService.release`.
pub fn release_task(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    reason: Option<&str>,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskClaim])?;
    let tasks = &world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let actor = stamp.actor.to_string();
    let plan = plan_release(task, caller_of(stamp, &actor), reason)?;
    let (status_id, _) = stamp.ids().next();
    let moved = step(
        Applied {
            decision: Decision::new(()),
            ids: stamp.ids(),
            tasks: tasks.clone(),
        },
        world,
        stamp,
        task.id,
        TaskStatus::Ready,
        Change {
            reason: Some(&plan.reason),
            ..Change::default()
        },
    )?;
    let (event_id, _) = moved.ids.next();
    let released = EventDraft::new(
        stamp,
        event_id,
        Some(task.workspace_id),
        task.id,
        EventPayload::TaskReleased(TaskReleased {
            previous_claimant: plan.previous_claimant,
            reason: plan.event_reason,
        }),
    )
    .caused_by(status_id);
    let decision = moved.decision.event(released);
    let state = tasks.apply_all(decision.writes.iter());
    let after = state.task(task.id).ok_or_else(|| gone(task.id))?;
    let out = task_out(&state, after, None);
    Ok(decision.with_reply(out))
}

/// The facts about a task that decide which verification checks apply to it.
pub fn scope_of(tasks: &TaskSlice, task: &wg_model::Task) -> TaskScope {
    TaskScope {
        workspace_id: task.workspace_id,
        task_id: task.id,
        project_ids: tasks
            .projects_of(task.id)
            .map(|row| row.project_id)
            .collect(),
        repository_ids: tasks
            .repositories_of(task.id)
            .map(|row| row.repository_id)
            .collect(),
    }
}

/// `_complete_in_session`: moves a claimed or running task to verification or review, records
/// `task.completed` and, with a summary, the result comment.
pub fn complete_in(
    start: Applied,
    reported: &Reported,
    stamp: &Stamp,
    task: &wg_model::Task,
    summary: &str,
    attempt_given: Option<&str>,
) -> Result<Applied, WgError> {
    let actor = stamp.actor.to_string();
    let has_checks =
        !applicable_checks(&reported.configs, &scope_of(&start.tasks, task))?.is_empty();
    let current = start.tasks.task(task.id).ok_or_else(|| gone(task.id))?;
    let plan = plan_complete(current, caller_of(stamp, &actor), has_checks)?;
    // The attempt is named only in the events, so a missing one is found once the plan holds.
    let attempt = named_attempt(reported, attempt_given)?;
    let (status_id, _) = start.ids.next();
    let moved = step(
        start,
        &reported.world,
        stamp,
        task.id,
        plan.target,
        Change {
            reason: Some(REASON_COMPLETION),
            attempt_id: Some(attempt),
            ..Change::default()
        },
    )?;
    let (event_id, ids) = moved.ids.next();
    let completed = EventDraft::new(
        stamp,
        event_id,
        Some(task.workspace_id),
        task.id,
        EventPayload::TaskCompleted(TaskCompleted {
            summary: summary.chars().take(5000).collect(),
            next_status: plan.target,
            verification_configured: plan.verification_configured,
        }),
    )
    .with_attempt(attempt)
    .caused_by(status_id);
    let decision = moved.decision.event(completed);
    if summary.is_empty() {
        return Ok(Applied {
            decision,
            ids,
            tasks: moved.tasks,
        });
    }
    let comment = comment_step(
        stamp,
        ids,
        task.id,
        task.workspace_id,
        CommentKind::Result,
        summary,
        &Map::new(),
        attempt,
        Some(status_id),
    );
    let tasks = moved.tasks.apply_all(comment.decision.writes.iter());
    Ok(Applied {
        decision: decision.then(comment.decision),
        ids: comment.ids,
        tasks,
    })
}

/// The attempt a report names, which may be absent: its id, or `not_found`.
fn named_attempt(reported: &Reported, given: Option<&str>) -> Result<Option<Id>, WgError> {
    given
        .map(|text| attempt_of(text, reported.attempt.as_ref()).map(|attempt| attempt.id))
        .transpose()
}

/// `TaskService.complete`: agent-declared completion, never to accepted.
pub fn complete_task(
    reported: &Reported,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    read: &CompleteRead,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskComplete])?;
    let tasks = &reported.world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let done = complete_in(
        Applied {
            decision: Decision::new(()),
            ids: stamp.ids(),
            tasks: tasks.clone(),
        },
        reported,
        stamp,
        task,
        &read.summary,
        read.attempt_id.as_deref(),
    )?;
    let after = done.tasks.task(task.id).ok_or_else(|| gone(task.id))?;
    let out = task_out(&done.tasks, after, None);
    Ok(done.decision.with_reply(out))
}

/// `finish_attempt` for a self-reported outcome: the attempt row and its terminal event.
fn finish_attempt(
    stamp: &Stamp,
    ids: IdSeq,
    attempt: &AgentAttempt,
    read: &ResultRead,
) -> Result<(Decision<()>, IdSeq), WgError> {
    let target = if read.succeeded {
        AttemptStatus::Succeeded
    } else {
        AttemptStatus::Failed
    };
    let FinishPlan::Apply { .. } = finish_plan(attempt.status, target)? else {
        return Ok((Decision::new(()), ids));
    };
    let usage = read
        .usage
        .as_ref()
        .map(sanitize_usage)
        .filter(|clean| !clean.is_empty());
    let elapsed = attempt.started_at.map(|started| {
        i64::try_from(
            (stamp.at.to_offset_datetime() - started.to_offset_datetime()).whole_milliseconds(),
        )
        .unwrap_or(i64::MAX)
    });
    let finished = AgentAttempt {
        status: target,
        finished_at: Some(stamp.at),
        exit_reason: Some("agent_reported".to_owned()),
        exit_code: None,
        summary: if read.summary.is_empty() {
            attempt.summary.clone()
        } else {
            Some(read.summary.chars().take(50_000).collect())
        },
        usage: usage.clone().unwrap_or_else(|| attempt.usage.clone()),
        usage_source: if usage.is_some() {
            Some("agent_reported".to_owned())
        } else {
            attempt.usage_source.clone()
        },
        cost_usd: read.cost_usd.map(Real).or(attempt.cost_usd),
        cost_source: if read.cost_usd.is_some() {
            Some("agent_reported".to_owned())
        } else {
            attempt.cost_source.clone()
        },
        duration_ms: duration_ms(None, elapsed).or(attempt.duration_ms),
        updated_at: stamp.at,
        ..attempt.clone()
    };
    let (event_id, ids) = ids.next();
    let body = AttemptFinished {
        task_id: attempt.task_id,
        from: attempt.status,
        status: target,
        exit_reason: Some("agent_reported".to_owned()),
        exit_code: None,
        duration_ms: finished.duration_ms,
        error: None,
    };
    let payload = if read.succeeded {
        EventPayload::AttemptSucceeded(body)
    } else {
        EventPayload::AttemptFailed(body)
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(attempt.workspace_id),
        attempt.id,
        payload,
    )
    .with_attempt(Some(attempt.id));
    Ok((
        Decision::new(())
            .write(Write::update(finished))
            .event(event),
        ids,
    ))
}

/// `TaskService.report_result`: an attempt's self-reported outcome. Success still needs
/// verification and acceptance.
pub fn report_result(
    reported: &Reported,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    read: &ResultRead,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskComplete])?;
    let tasks = &reported.world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let attempt = owned_attempt(
        task.id,
        read.attempt_id.as_deref(),
        reported.attempt.as_ref(),
    )?;
    let (finishing, ids) = match (&reported.attempt, attempt) {
        (Some(found), Some(_)) => finish_attempt(stamp, stamp.ids(), found, read)?,
        _ => (Decision::new(()), stamp.ids()),
    };
    let actor = stamp.actor.to_string();
    let plan = plan_report_result(
        task,
        caller_of(stamp, &actor),
        read.succeeded,
        &read.summary,
        !applicable_checks(&reported.configs, &scope_of(tasks, task))?.is_empty(),
    )?;
    let start = Applied {
        decision: finishing,
        ids,
        tasks: tasks.clone(),
    };
    let after_completion = match &plan.completion {
        Some(_) => complete_in(
            start,
            reported,
            stamp,
            task,
            &read.summary,
            read.attempt_id.as_deref(),
        )?,
        None => start,
    };
    let after_comment = match (&plan.completion, &plan.comment) {
        (None, Some(result)) => {
            let data = if result.failed {
                json!({"outcome": "failed"})
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
            } else {
                Map::new()
            };
            let comment = comment_step(
                stamp,
                after_completion.ids,
                task.id,
                task.workspace_id,
                CommentKind::Result,
                &result.body,
                &data,
                attempt,
                None,
            );
            Applied {
                tasks: after_completion
                    .tasks
                    .apply_all(comment.decision.writes.iter()),
                decision: after_completion.decision.then(comment.decision),
                ids: comment.ids,
            }
        }
        _ => after_completion,
    };
    let done = if plan.fail {
        step(
            after_comment,
            &reported.world,
            stamp,
            task.id,
            TaskStatus::Failed,
            Change {
                reason: Some(REASON_ATTEMPT_FAILED),
                ..Change::default()
            },
        )?
    } else {
        after_comment
    };
    let after = done.tasks.task(task.id).ok_or_else(|| gone(task.id))?;
    let out = task_out(&done.tasks, after, None);
    Ok(done.decision.with_reply(out))
}
