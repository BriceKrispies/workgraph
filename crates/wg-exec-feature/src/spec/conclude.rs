//! How a run ends (`RunService.conclude_attempt`, `_finalize_failure` and `_maybe_escalate` of
//! `orchestration/runs.py`) as decisions: the attempt finished or kept running, the task moved on,
//! the task flagged for review after too many failures, and the run records (log, telemetry) the
//! worker adds in the same transaction.

use serde_json::{Map, Value, json};
use wg_clispec::args::python;
use wg_exec::attempt::{counts_as_failure, escalates, is_terminal, task_after_attempt_end};
use wg_exec::run::{CaptureFacts, Conclusion, completes_task, conclude, kept_summary, task_reason};
use wg_json::Value as Wire;
use wg_model::entity::{AgentAttempt, Artifact, Real, TaskTag};
use wg_model::event::{EventDraft, EventPayload, TaskEscalated};
use wg_model::stamp::{IdSeq, Stamp};
use wg_model::{Decision, Write};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::transition::{Change, transition};
use wg_tasks_feature::spec::bodies::CompleteRead;
use wg_tasks_feature::spec::claims::{Reported, complete_task};
use wg_types::{AttemptStatus, Id, TaskStatus, WgError};
use wg_wire::convention::{dict, int, text};

use crate::spec::attempts::{Finish, finish};

/// The tag an escalated task carries.
const NEEDS_REVIEW: &str = "needs-review";

/// What the worker records about the run itself, in the transaction that ends the attempt.
#[derive(Debug, Clone, Default)]
pub struct RunRecord {
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// Merged over the attempt's telemetry.
    pub telemetry: Map<String, Value>,
    /// The registered agent log.
    pub log: Option<Artifact>,
}

/// The attempt with the run's records applied.
pub fn with_record(attempt: &AgentAttempt, record: &RunRecord, stamp: &Stamp) -> AgentAttempt {
    AgentAttempt {
        session_id: record
            .session_id
            .clone()
            .filter(|id| !id.is_empty())
            .or_else(|| attempt.session_id.clone()),
        model: record
            .model
            .clone()
            .filter(|name| !name.is_empty())
            .or_else(|| attempt.model.clone()),
        telemetry: attempt
            .telemetry
            .iter()
            .chain(record.telemetry.iter())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        updated_at: stamp.at,
        ..attempt.clone()
    }
}

/// The facts the ending decisions read about the task and its attempts.
pub struct EndFacts<'a> {
    pub world: &'a TaskWorld,
    /// Every attempt of the task, as stored.
    pub attempts: &'a [AgentAttempt],
    /// `execution.max_attempts_per_task`.
    pub max_attempts: i64,
}

/// An attempt end: its changes, and the ids still free.
pub struct Ended {
    pub decision: Decision<()>,
    pub ids: IdSeq,
}

/// The error a collaborator that is not ported yet answers with.
pub fn blocked(what: &str) -> WgError {
    WgError::workgraph(format!("{what} is not ported yet"))
}

fn gone(what: &str, id: Id) -> WgError {
    WgError::not_found(format!("{what} {id} not found"))
}

/// `_maybe_escalate`: a failed task whose failed attempts reached the limit is flagged for review.
fn escalate(
    facts: &EndFacts<'_>,
    tasks: &wg_model::TaskSlice,
    attempts: &[AgentAttempt],
    stamp: &Stamp,
    ids: IdSeq,
    task_id: Id,
) -> (Decision<()>, IdSeq) {
    let Some(task) = tasks.task(task_id) else {
        return (Decision::new(()), ids);
    };
    let failures = attempts
        .iter()
        .filter(|row| row.task_id == task_id && counts_as_failure(row.status))
        .count();
    let flagged = tasks.tags_of(task_id).any(|tag| tag == NEEDS_REVIEW);
    let limit = facts.max_attempts;
    if !escalates(
        task.status,
        u64::try_from(failures).unwrap_or(u64::MAX),
        u64::try_from(limit).unwrap_or(0),
        flagged,
    ) {
        return (Decision::new(()), ids);
    }
    let (event_id, ids) = ids.next();
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(task.workspace_id),
        task_id,
        EventPayload::TaskEscalated(TaskEscalated {
            failed_attempts: i64::try_from(failures).unwrap_or(i64::MAX),
            limit,
            reason: "attempt limit reached".to_owned(),
        }),
    );
    (
        Decision::new(())
            .write(Write::insert(TaskTag {
                task_id,
                tag: NEEDS_REVIEW.to_owned(),
            }))
            .event(event),
        ids,
    )
}

/// How an end treats the attempt and the escalation check.
#[derive(Debug, Clone, Copy)]
pub struct EndMode {
    /// The attempt is left alone when it already finished (`_finalize_failure`), instead of being
    /// finished again (which refuses a different status).
    pub skip_if_terminal: bool,
    /// The task is checked for escalation whether or not it was moved (`conclude_attempt`), instead
    /// of only when it was (`_finalize_failure`).
    pub escalate_outside: bool,
}

/// Ends `attempt` as `status`, moves its held task (ready after a cancel, failed after anything
/// else), and flags the task for review when the failures reached the limit. `reason` is the text
/// of the task transition.
#[allow(clippy::too_many_arguments)]
pub fn end_attempt(
    facts: &EndFacts<'_>,
    stamp: &Stamp,
    ids: IdSeq,
    attempt: &AgentAttempt,
    status: AttemptStatus,
    args: &Finish,
    reason: &str,
    mode: EndMode,
) -> Result<Ended, WgError> {
    let (event_id, ids) = ids.next();
    let finished = if mode.skip_if_terminal && is_terminal(attempt.status) {
        None
    } else {
        finish(stamp, event_id, attempt, status, args)?
    };
    let after = finished
        .as_ref()
        .map_or_else(|| attempt.clone(), |(row, _)| row.clone());
    let base = match finished {
        Some((row, event)) => Decision::new(()).write(Write::update(row)).event(event),
        None => Decision::new(()),
    };
    let task = facts
        .world
        .tasks
        .task(attempt.task_id)
        .ok_or_else(|| gone("task", attempt.task_id))?;
    let target = task_after_attempt_end(task.status, status);
    let moved = target
        .map(|to| {
            transition(
                &facts.world.tasks,
                &facts.world.stacks,
                stamp,
                ids,
                task.id,
                to,
                Change {
                    reason: Some(reason),
                    ..Change::default()
                },
            )
        })
        .transpose()?;
    let (decision, tasks, ids) = match moved {
        Some(applied) => (base.then(applied.decision), applied.tasks, applied.ids),
        None => (base, facts.world.tasks.clone(), ids),
    };
    let attempts: Vec<AgentAttempt> = facts
        .attempts
        .iter()
        .map(|row| {
            if row.id == after.id {
                after.clone()
            } else {
                row.clone()
            }
        })
        .collect();
    let (flag, ids) = if target.is_some() || mode.escalate_outside {
        escalate(facts, &tasks, &attempts, stamp, ids, task.id)
    } else {
        (Decision::new(()), ids)
    };
    Ok(Ended {
        decision: decision.then(flag),
        ids,
    })
}

/// `_finalize_failure`: the attempt failed or cancelled before or while the run, with the error as
/// the task's transition reason (cut to 200 characters).
pub fn finalize_failure(
    facts: &EndFacts<'_>,
    stamp: &Stamp,
    attempt: &AgentAttempt,
    end: &wg_exec::run::EarlyEnd,
) -> Result<Decision<()>, WgError> {
    let ended = end_attempt(
        facts,
        stamp,
        stamp.ids(),
        attempt,
        end.status,
        &Finish {
            exit_reason: Some(end.exit_reason.clone()),
            error: Some(end.error.clone()),
            ..Finish::default()
        },
        &end.reason(),
        EndMode {
            skip_if_terminal: true,
            escalate_outside: false,
        },
    )?;
    Ok(ended.decision)
}

/// What the agent reported, as the conclusion reads it.
#[derive(Debug, Clone)]
pub struct AgentReport {
    /// The agent's result status: `succeeded`, `failed`, `timed_out` or `cancelled`.
    pub status: String,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub exit_code: Option<i64>,
    pub duration_ms: Option<i64>,
    /// `usage`, `usage_source`, `cost_usd` and `cost_source` as `usage_fields` builds them.
    pub usage_fields: Map<String, Value>,
}

/// Whether the first stage of a conclusion left the attempt running toward completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The agent's success stands.
    Succeeded,
    /// The attempt ended.
    Ended,
}

fn usage_part(fields: &Map<String, Value>) -> Finish {
    Finish {
        usage: fields.get("usage").and_then(Value::as_object).cloned(),
        usage_source: fields
            .get("usage_source")
            .and_then(Value::as_str)
            .map(str::to_owned),
        cost_usd: fields.get("cost_usd").and_then(Value::as_f64),
        cost_source: fields
            .get("cost_source")
            .and_then(Value::as_str)
            .map(str::to_owned),
        ..Finish::default()
    }
}

/// The first transaction of `conclude_attempt`: records the run, then either keeps the agent's
/// success on the attempt (usage, cost, summary, exit code, duration) or ends the attempt and
/// its task.
pub fn conclude_first(
    facts: &EndFacts<'_>,
    stamp: &Stamp,
    attempt: &AgentAttempt,
    report: &AgentReport,
    capture: &CaptureFacts,
    record: Option<&RunRecord>,
) -> Result<Decision<Stage>, WgError> {
    let recorded = record.map_or_else(
        || attempt.clone(),
        |found| with_record(attempt, found, stamp),
    );
    let log = record.and_then(|found| found.log.clone()).map_or_else(
        || Decision::new(()),
        |row| Decision::new(()).write(Write::insert(row)),
    );
    let conclusion = conclude(&report.status, report.error.as_deref(), capture)?;
    match conclusion {
        Conclusion::Succeeded => {
            let fields = &report.usage_fields;
            let kept = AgentAttempt {
                usage: fields
                    .get("usage")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_else(|| recorded.usage.clone()),
                usage_source: fields
                    .get("usage_source")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| recorded.usage_source.clone()),
                cost_usd: fields
                    .get("cost_usd")
                    .and_then(Value::as_f64)
                    .map(Real)
                    .or(recorded.cost_usd),
                cost_source: fields
                    .get("cost_source")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| recorded.cost_source.clone()),
                summary: kept_summary(report.summary.as_deref(), recorded.summary.as_deref()),
                exit_code: report.exit_code,
                duration_ms: report.duration_ms,
                updated_at: stamp.at,
                ..recorded
            };
            Ok(log
                .then(Decision::new(()).write(Write::update(kept)))
                .with_reply(Stage::Succeeded))
        }
        Conclusion::Failed {
            attempt: ended,
            exit_reason,
            error,
        } => {
            let task_number = recorded.number;
            let args = Finish {
                exit_reason: Some(exit_reason),
                exit_code: report.exit_code,
                error: Some(error),
                summary: report.summary.clone(),
                duration_ms: report.duration_ms,
                ..usage_part(&report.usage_fields)
            };
            let ended = end_attempt(
                facts,
                stamp,
                stamp.ids(),
                &recorded,
                ended,
                &args,
                &task_reason(task_number, ended),
                EndMode {
                    skip_if_terminal: false,
                    escalate_outside: true,
                },
            )?;
            // The run records are part of `recorded`, the row the ending finishes.
            Ok(log.then(ended.decision).with_reply(Stage::Ended))
        }
    }
}

/// The second transaction: completes the held task from the agent's summary, and, when the task
/// does not wait for verification, succeeds the attempt at once. The reply says whether the task
/// now awaits verification.
pub fn complete_stage(
    reported: &Reported,
    attempt: &AgentAttempt,
    task_id: Id,
    summary: &str,
    exit_code: Option<i64>,
    duration_ms: Option<i64>,
    stamp: &Stamp,
) -> Result<Decision<bool>, WgError> {
    let task = reported
        .world
        .tasks
        .task(task_id)
        .ok_or_else(|| gone("task", task_id))?;
    // `complete_task` draws its ids from slot 0; the attempt's event uses other entropy.
    let other = Stamp {
        entropy: stamp.entropy.map(|byte| byte ^ 0x33),
        ..stamp.clone()
    };
    let (completed, tasks) = if completes_task(task.status) {
        let made = complete_task(
            reported,
            stamp,
            None,
            &task_id.to_string(),
            &CompleteRead {
                summary: summary.to_owned(),
                attempt_id: Some(attempt.id.to_string()),
            },
        )?
        .with_reply(());
        let tasks = reported.world.tasks.apply_all(made.writes.iter());
        (made, tasks)
    } else {
        (Decision::new(()), reported.world.tasks.clone())
    };
    let waits = tasks
        .task(task_id)
        .is_some_and(|row| row.status == TaskStatus::AwaitingVerification);
    if waits {
        return Ok(completed.with_reply(true));
    }
    let (event_id, _) = other.ids().next();
    let done = finish(
        &other,
        event_id,
        attempt,
        AttemptStatus::Succeeded,
        &Finish {
            exit_reason: Some("agent_succeeded".to_owned()),
            exit_code,
            duration_ms,
            ..Finish::default()
        },
    )?;
    let finished = match done {
        Some((row, event)) => Decision::new(()).write(Write::update(row)).event(event),
        None => Decision::new(()),
    };
    Ok(completed.then(finished).with_reply(false))
}

/// The attempt succeeded after a gate or a verification: `agent_succeeded`.
pub fn succeed(
    attempt: &AgentAttempt,
    exit_code: Option<i64>,
    duration_ms: Option<i64>,
    stamp: &Stamp,
) -> Result<Decision<()>, WgError> {
    let (event_id, _) = stamp.ids().next();
    let done = finish(
        stamp,
        event_id,
        attempt,
        AttemptStatus::Succeeded,
        &Finish {
            exit_reason: Some("agent_succeeded".to_owned()),
            exit_code,
            duration_ms,
            ..Finish::default()
        },
    )?;
    Ok(match done {
        Some((row, event)) => Decision::new(()).write(Write::update(row)).event(event),
        None => Decision::new(()),
    })
}

/// A verification error that failed the attempt: `verification_error` with the message.
pub fn fail_verification(
    attempt: &AgentAttempt,
    message: &str,
    stamp: &Stamp,
) -> Result<Decision<()>, WgError> {
    let Some(end) = wg_exec::run::verification_error_end(attempt.status, message) else {
        return Ok(Decision::new(()));
    };
    let (event_id, _) = stamp.ids().next();
    let done = finish(
        stamp,
        event_id,
        attempt,
        end.status,
        &Finish {
            exit_reason: Some(end.exit_reason),
            error: Some(end.error),
            ..Finish::default()
        },
    )?;
    Ok(match done {
        Some((row, event)) => Decision::new(()).write(Write::update(row)).event(event),
        None => Decision::new(()),
    })
}

/// The escalation check alone, after verification (`_maybe_escalate` in its own transaction).
pub fn escalate_task(facts: &EndFacts<'_>, stamp: &Stamp, task_id: Id) -> Decision<()> {
    escalate(
        facts,
        &facts.world.tasks,
        facts.attempts,
        stamp,
        stamp.ids(),
        task_id,
    )
    .0
}

/// What `capture_outcome` found, as `conclude_attempt` reports it.
#[derive(Debug, Clone, Default)]
pub struct Captured {
    pub manifest_id: Option<Id>,
    pub modified: Vec<String>,
    pub violations: Vec<(String, usize, usize)>,
    pub commits: Vec<(String, Vec<String>)>,
    pub uncommitted: Vec<(String, Vec<String>)>,
    /// The error that stopped the capture.
    pub error: Option<String>,
    /// The error that stopped linking the attempt's commits.
    pub link_error: Option<String>,
}

impl Captured {
    /// The violations as `outcome['violations']` prints in the failure text, a Python list repr.
    pub fn violations_text(&self) -> Option<String> {
        (!self.violations.is_empty()).then(|| {
            let rows: Vec<Wire> = self
                .violations
                .iter()
                .map(|(repository, commits, files)| {
                    dict(vec![
                        ("repository", text(repository)),
                        ("commits", int(i64::try_from(*commits).unwrap_or(i64::MAX))),
                        (
                            "uncommitted_files",
                            int(i64::try_from(*files).unwrap_or(i64::MAX)),
                        ),
                    ])
                })
                .collect();
            python(&Wire::List(rows))
        })
    }

    /// What the conclusion decides on.
    pub fn facts(&self) -> CaptureFacts {
        CaptureFacts {
            violations: self.violations_text(),
            capture_error: self.error.clone(),
        }
    }

    /// The outcome dict without its `commits`, as `conclude_attempt` returns it.
    pub fn report(&self) -> Value {
        match (&self.error, self.manifest_id) {
            (Some(error), _) => json!({"error": error}),
            (None, manifest) => {
                let mut report = Map::new();
                report.insert(
                    "outcome_manifest_id".to_owned(),
                    manifest.map_or(Value::Null, |id| json!(id.to_string())),
                );
                report.insert("modified".to_owned(), json!(self.modified));
                report.insert(
                    "violations".to_owned(),
                    Value::Array(
                        self.violations
                            .iter()
                            .map(|(repository, commits, files)| {
                                json!({
                                    "repository": repository,
                                    "commits": commits,
                                    "uncommitted_files": files,
                                })
                            })
                            .collect(),
                    ),
                );
                report.insert(
                    "uncommitted".to_owned(),
                    Value::Object(
                        self.uncommitted
                            .iter()
                            .map(|(repo, files)| (repo.clone(), json!(files)))
                            .collect(),
                    ),
                );
                if let Some(link) = &self.link_error {
                    report.insert("link_error".to_owned(), json!(link));
                }
                Value::Object(report)
            }
        }
    }
}
