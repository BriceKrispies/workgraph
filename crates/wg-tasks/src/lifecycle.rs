//! Task and attempt lifecycle state machines (GV-01). Mirrors `core/policies/lifecycle.py`.
//!
//! Task states describe the work item; attempt states describe one execution of it. The two
//! machines are separate: an attempt can succeed while its task still awaits verification,
//! review or acceptance.
//!
//! Every target list is a `match` over the source status, so the tables are constants with no
//! collection type. Error details list allowed targets sorted by wire string, as Python's
//! `sorted(...)` does.

use serde_json::{Value, json};
use wg_types::{AttemptStatus, TaskStatus, WgError};

use AttemptStatus as A;
use TaskStatus as S;

/// Targets of each task status, as `TASK_TRANSITIONS` in the Python module.
pub fn task_targets(from: S) -> &'static [S] {
    match from {
        S::Backlog => &[S::Ready, S::Blocked, S::Cancelled],
        S::Ready => &[S::Backlog, S::Blocked, S::Claimed, S::Running, S::Cancelled],
        S::Blocked => &[S::Backlog, S::Ready, S::Cancelled],
        S::Claimed => &[
            S::Ready,
            S::Blocked,
            S::Running,
            S::AwaitingVerification,
            S::AwaitingReview,
            S::Failed,
            S::Cancelled,
        ],
        S::Running => &[
            S::Ready,
            S::Blocked,
            S::Claimed,
            S::AwaitingVerification,
            S::AwaitingReview,
            S::Failed,
            S::Cancelled,
        ],
        S::AwaitingVerification => &[
            S::AwaitingReview,
            S::Accepted,
            S::Failed,
            S::Running,
            S::Ready,
            S::Claimed,
            S::Cancelled,
        ],
        S::AwaitingReview => &[
            S::Accepted,
            S::Failed,
            S::Running,
            S::Ready,
            S::Claimed,
            S::Cancelled,
        ],
        S::Failed => &[
            S::Ready,
            S::Backlog,
            S::Claimed,
            S::Running,
            S::Blocked,
            S::Cancelled,
        ],
        // Reopening accepted or cancelled work is allowed and recorded as `task.reopened`.
        S::Accepted => &[S::Ready, S::Backlog],
        S::Cancelled => &[S::Backlog, S::Ready],
    }
}

/// Targets of each attempt status, as `ATTEMPT_TRANSITIONS` in the Python module.
pub fn attempt_targets(from: A) -> &'static [A] {
    match from {
        A::Queued => &[
            A::Preparing,
            A::Running,
            A::Cancelled,
            A::Abandoned,
            A::Failed,
        ],
        A::Preparing => &[A::Running, A::Failed, A::Cancelled, A::Abandoned],
        A::Running => &[
            A::Verifying,
            A::Succeeded,
            A::Failed,
            A::Cancelled,
            A::TimedOut,
            A::Abandoned,
        ],
        A::Verifying => &[A::Succeeded, A::Failed, A::Cancelled, A::Abandoned],
        A::Succeeded | A::Failed | A::Cancelled | A::TimedOut | A::Abandoned => &[],
    }
}

/// Entering these statuses is reserved for the acceptance service, unless `privileged`.
pub const RESTRICTED_TARGETS: &[TaskStatus] = &[S::Accepted];

/// Statuses that require every dependency to be satisfied.
pub const REQUIRES_DEPENDENCIES: &[TaskStatus] = &[S::Ready, S::Claimed, S::Running];

/// A tracker closing its issue closes the mirrored task from any status.
pub const TRACKER_CLOSE_TARGETS: &[TaskStatus] = &[S::Accepted, S::Cancelled];

/// Every task status in wire-string order, so allowed-target lists come out sorted.
const TASK_STATUS_BY_WIRE: [TaskStatus; 10] = [
    S::Accepted,
    S::AwaitingReview,
    S::AwaitingVerification,
    S::Backlog,
    S::Blocked,
    S::Cancelled,
    S::Claimed,
    S::Failed,
    S::Ready,
    S::Running,
];

/// Every attempt status in wire-string order.
const ATTEMPT_STATUS_BY_WIRE: [AttemptStatus; 9] = [
    A::Abandoned,
    A::Cancelled,
    A::Failed,
    A::Preparing,
    A::Queued,
    A::Running,
    A::Succeeded,
    A::TimedOut,
    A::Verifying,
];

/// Whether a task in `status` must have every dependency satisfied.
pub fn requires_dependencies(status: TaskStatus) -> bool {
    REQUIRES_DEPENDENCIES.contains(&status)
}

/// Checks one task status change. `privileged` is the acceptance service's flag; `tracker` marks
/// a tracker-driven close.
pub fn check_task_transition(
    current: TaskStatus,
    target: TaskStatus,
    privileged: bool,
    tracker: bool,
) -> Result<(), WgError> {
    if current == target {
        return Ok(());
    }
    if tracker && TRACKER_CLOSE_TARGETS.contains(&target) {
        return Ok(());
    }
    if RESTRICTED_TARGETS.contains(&target) && !privileged {
        return Err(WgError::invalid_transition(format!(
                "tasks enter {target} only through acceptance (verification or recorded manual acceptance)"
            ))
        .detail("from", current.as_str())
        .detail("to", target.as_str()));
    }
    let targets = task_targets(current);
    if !targets.contains(&target) {
        let allowed: Vec<Value> = TASK_STATUS_BY_WIRE
            .iter()
            .filter(|status| targets.contains(*status))
            .map(|status| json!(status.as_str()))
            .collect();
        return Err(WgError::invalid_transition(format!(
            "cannot move task from {current} to {target}"
        ))
        .detail("from", current.as_str())
        .detail("to", target.as_str())
        .detail("allowed", Value::Array(allowed)));
    }
    Ok(())
}

/// Checks one attempt status change.
pub fn check_attempt_transition(
    current: AttemptStatus,
    target: AttemptStatus,
) -> Result<(), WgError> {
    if current == target {
        return Ok(());
    }
    let targets = attempt_targets(current);
    if !targets.contains(&target) {
        let allowed: Vec<Value> = ATTEMPT_STATUS_BY_WIRE
            .iter()
            .filter(|status| targets.contains(*status))
            .map(|status| json!(status.as_str()))
            .collect();
        return Err(WgError::invalid_transition(format!(
            "cannot move attempt from {current} to {target}"
        ))
        .detail("from", current.as_str())
        .detail("to", target.as_str())
        .detail("allowed", Value::Array(allowed)));
    }
    Ok(())
}
