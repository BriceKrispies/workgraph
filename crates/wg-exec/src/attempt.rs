//! Attempt decisions (CORE-11): the attempt state machine, the external `attempt start`, `submit`
//! and `abandon` checks, the conclusion rules shared by managed and external runs, and escalation.
//!
//! Mirrors the attempt parts of `core/services/attempts.py` and `orchestration/runs.py`. Each
//! function is pure: the shell reads the rows, asks here what to do, and applies the answer with
//! its own writes and events. The attempt transition table itself lives in `wg_tasks::lifecycle`
//! (GV-01) and is re-exported here, so there is one copy of it.

use serde_json::{Map, Value, json};
use wg_types::{AttemptStatus as A, ErrorCode, TaskStatus as S, WgError};

pub use wg_tasks::lifecycle::{attempt_targets, check_attempt_transition};

/// Statuses in which an attempt has finished (`TERMINAL_ATTEMPT_STATUSES`).
pub const TERMINAL_STATUSES: &[A] = &[
    A::Succeeded,
    A::Failed,
    A::Cancelled,
    A::TimedOut,
    A::Abandoned,
];

/// Statuses that hold a task's single active attempt (the external start check).
pub const ACTIVE_STATUSES: &[A] = &[A::Queued, A::Preparing, A::Running, A::Verifying];

/// Usage keys accepted from provider telemetry (`USAGE_KEYS`).
pub const USAGE_KEYS: &[&str] = &[
    "input_tokens",
    "output_tokens",
    "cache_creation_input_tokens",
    "cache_read_input_tokens",
    "total_tokens",
    "num_turns",
];

/// The tag an escalated task carries (`_maybe_escalate`).
pub const NEEDS_REVIEW_TAG: &str = "needs-review";

/// The event an escalation records (`_maybe_escalate`).
pub const ESCALATED_EVENT: &str = "task.escalated";

/// Whether `status` is a finished attempt status.
pub fn is_terminal(status: A) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

/// Whether an attempt in `status` is still in flight and so holds its task.
pub fn is_active(status: A) -> bool {
    ACTIVE_STATUSES.contains(&status)
}

/// Whether an attempt ending in `status` counts toward its task's failure total.
pub fn counts_as_failure(status: A) -> bool {
    matches!(status, A::Failed | A::TimedOut)
}

/// The number of a task's next attempt, given the numbers of its existing attempts.
pub fn next_attempt_number(existing: &[i64]) -> i64 {
    existing.iter().copied().max().unwrap_or(0) + 1
}

/// What `finish_attempt` does with a requested terminal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishPlan {
    /// The attempt already finished with this status: nothing is written.
    Unchanged,
    /// Write the change, then record the event named here.
    Apply { event: &'static str },
}

/// Decides a request to finish an attempt that is currently `current` with status `target`.
pub fn finish_plan(current: A, target: A) -> Result<FinishPlan, WgError> {
    if !is_terminal(target) {
        return Err(validation(format!(
            "{target} is not a terminal attempt status"
        )));
    }
    if is_terminal(current) {
        if current == target {
            return Ok(FinishPlan::Unchanged);
        }
        return Err(validation(format!("attempt already finished as {current}")));
    }
    check_attempt_transition(current, target)?;
    Ok(FinishPlan::Apply {
        event: finish_event(target),
    })
}

fn finish_event(target: A) -> &'static str {
    match target {
        A::Succeeded => "attempt.succeeded",
        A::Cancelled => "attempt.cancelled",
        // Failed, TimedOut and Abandoned all record as a failure.
        _ => "attempt.failed",
    }
}

/// The duration an attempt records when it finishes: the reported value, else the elapsed time
/// since it started, never negative. `elapsed_since_start` is `None` when it never started.
pub fn duration_ms(reported: Option<i64>, elapsed_since_start: Option<i64>) -> Option<i64> {
    reported.or_else(|| elapsed_since_start.map(|ms| ms.max(0)))
}

/// Keeps the usage counters an attempt may record (`sanitize_usage`): known keys with
/// non-negative numbers, truncated to whole numbers. Anything else is dropped rather than guessed.
pub fn sanitize_usage(usage: &Map<String, Value>) -> Map<String, Value> {
    usage
        .iter()
        .filter(|(key, _)| USAGE_KEYS.contains(&key.as_str()))
        .filter_map(|(key, value)| whole_count(value).map(|whole| (key.clone(), json!(whole))))
        .collect()
}

fn whole_count(value: &Value) -> Option<u64> {
    let number = value.as_number()?;
    if let Some(whole) = number.as_u64() {
        return Some(whole);
    }
    let float = number.as_f64()?;
    (float >= 0.0 && float.is_finite()).then(|| float.trunc() as u64)
}

/// What `start_external` does with the task before it prepares worktrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartPlan {
    /// The task is ready and must be claimed for the actor first. A task already claimed by the
    /// actor is continued without a new claim.
    pub claim_first: bool,
}

/// Checks an external `attempt start` on a task. `actor` is the acting identity, `claimed_by_actor`
/// says whether it holds the task's claim, `holder` names another claimant, and
/// `active_attempt_number` is the number of an attempt that already holds the task, if any.
pub fn start_plan(
    actor: &str,
    task_key: &str,
    task: S,
    claimed_by_actor: bool,
    holder: Option<&str>,
    active_attempt_number: Option<i64>,
) -> Result<StartPlan, WgError> {
    let claim_first = match task {
        S::Ready => true,
        S::Claimed if claimed_by_actor => false,
        _ => {
            let by = match (task, holder) {
                (S::Claimed, Some(name)) => format!(" by {name}"),
                _ => String::new(),
            };
            return Err(validation(format!(
                "task {task_key} is {task}{by}; an external attempt needs a ready task or one claimed by {actor}"
            ))
            .with_detail("status", json!(task.as_str())));
        }
    };
    if let Some(number) = active_attempt_number {
        return Err(WgError::new(
            ErrorCode::InvalidTransition,
            format!("task {task_key} already has an active attempt #{number}"),
        ));
    }
    Ok(StartPlan { claim_first })
}

/// The task status once an external attempt's worktrees are prepared: a claimed task starts running.
pub fn task_after_prepare(task: S) -> S {
    if task == S::Claimed { S::Running } else { task }
}

/// The task status an attempt ending in `attempt` leaves behind (`conclude_attempt` and
/// `_finalize_failure`). A cancelled attempt returns the task to ready, any other ending fails it.
/// `None` when the task is neither claimed nor running and must be left alone.
pub fn task_after_attempt_end(task: S, attempt: A) -> Option<S> {
    if !matches!(task, S::Claimed | S::Running) {
        return None;
    }
    Some(if attempt == A::Cancelled {
        S::Ready
    } else {
        S::Failed
    })
}

/// Checks an external `submit` before the attempt is concluded. The agent's `requested` status
/// must be succeeded or failed, the attempt must be running or preparing with a managed worktree,
/// and the task must still be running or claimed.
pub fn submit_check(
    requested: &str,
    attempt_id: &str,
    attempt: A,
    has_worktree: bool,
    task_key: &str,
    task: S,
) -> Result<(), WgError> {
    if requested != "succeeded" && requested != "failed" {
        return Err(validation(format!(
            "status must be succeeded or failed, not {}",
            python_repr(requested)
        )));
    }
    if !matches!(attempt, A::Running | A::Preparing) {
        return Err(validation(format!(
            "attempt {attempt_id} is {attempt}; only a running attempt can be submitted"
        )));
    }
    if !has_worktree {
        return Err(validation(format!(
            "attempt {attempt_id} has no managed worktree; submit applies to attempts from `attempt start`"
        )));
    }
    if !matches!(task, S::Running | S::Claimed) {
        return Err(validation(format!(
            "task {task_key} is {task}; only a running task can be submitted"
        )));
    }
    Ok(())
}

/// Checks an external `abandon` and returns the task status to move to, if any. A reason is
/// required, only attempts started with `attempt start` qualify, and only a running or preparing
/// attempt can be abandoned. A claimed or running task returns to ready.
pub fn abandon_plan(
    external: bool,
    attempt_id: &str,
    attempt: A,
    reason: &str,
    task: S,
) -> Result<Option<S>, WgError> {
    if reason.trim().is_empty() {
        return Err(validation("abandoning an attempt needs a reason"));
    }
    if !external {
        return Err(validation(format!(
            "attempt {attempt_id} was not started with attempt start; only external attempts are abandoned"
        )));
    }
    if !matches!(attempt, A::Running | A::Preparing) {
        return Err(validation(format!(
            "attempt {attempt_id} is {attempt}; only a running attempt can be abandoned"
        )));
    }
    Ok(matches!(task, S::Claimed | S::Running).then_some(S::Ready))
}

/// The attempt status an unsuccessful agent result concludes with (`conclude_attempt`). A
/// reported `succeeded` lands here too when it is overturned, as failed. `None` for an unknown
/// status.
pub fn unsuccessful_status(status: &str) -> Option<A> {
    match status {
        "failed" | "succeeded" => Some(A::Failed),
        "timed_out" => Some(A::TimedOut),
        "cancelled" => Some(A::Cancelled),
        _ => None,
    }
}

/// Whether a conclusion stands as a success. A reported success is overturned by read-only policy
/// violations or by a failure to record the attempt's changes.
pub fn is_success(status: &str, violations: bool, capture_error: bool) -> bool {
    status == "succeeded" && !violations && !capture_error
}

/// The `exit_reason` recorded when a conclusion is unsuccessful.
pub fn failure_exit_reason(status: &str, capture_error: bool) -> String {
    if status != "succeeded" {
        status.to_owned()
    } else if capture_error {
        "outcome_capture_failed".to_owned()
    } else {
        "policy_violation".to_owned()
    }
}

/// Whether a failed attempt escalates its task to human review (`_maybe_escalate`): the task is
/// failed, its failed or timed-out attempts reached the limit, and it is not flagged already.
pub fn escalates(task: S, failures: u64, limit: u64, already_flagged: bool) -> bool {
    task == S::Failed && failures >= limit && !already_flagged
}

/// Python `repr()` of a short string: single quotes, or double quotes when it holds a single quote
/// and no double quote.
fn python_repr(text: &str) -> String {
    if text.contains('\'') && !text.contains('"') {
        format!("\"{text}\"")
    } else {
        format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

fn validation(message: impl Into<String>) -> WgError {
    WgError::new(ErrorCode::ValidationFailed, message)
}
