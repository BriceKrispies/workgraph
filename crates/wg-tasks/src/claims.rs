//! Claims, releases, lease expiry and completion of tasks (CORE-04). Mirrors `claim`,
//! `_claim_in_session`, `release`, `complete`, `_complete_in_session`, `report_result` and
//! `release_expired_claims` in `core/services/tasks.py`.
//!
//! Each function decides and returns a plan; the shell applies the status change through
//! [`check_task_transition`] and the dependency gate, then writes the event rows the plan names.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use time::Duration;
use wg_model::{Task, TaskSlice};
use wg_types::{ErrorCode, TaskStatus, Timestamp, WgError};

use crate::lifecycle::check_task_transition;

/// Actors with this prefix are system processes and may complete any claim.
pub const SYSTEM_ACTOR_PREFIX: &str = "system:";

/// Reason recorded when a claim is re-taken after its foreign claimant's lease expired.
pub const REASON_EXPIRED_CLAIM_RELEASED: &str = "expired claim released";
/// Reason recorded when the sweep releases an expired claim.
pub const REASON_LEASE_EXPIRED: &str = "claim lease expired";
/// Reason recorded by a release that gave no reason.
pub const REASON_RELEASED: &str = "released";
/// Reason recorded when completion moves the task to review or verification.
pub const REASON_COMPLETION: &str = "completion reported";
/// Reason recorded when an attempt reports failure and the task moves to failed.
pub const REASON_ATTEMPT_FAILED: &str = "attempt reported failure";

/// The longest lease a claim may ask for, in minutes (`ClaimRequest.lease_minutes`).
pub const MAX_LEASE_MINUTES: i64 = 7 * 24 * 60;

/// The caller's identity as the claim and completion checks need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caller<'a> {
    /// `ctx.actor`, such as `human:brice`.
    pub actor: &'a str,
    /// Whether the caller holds `task:admin`.
    pub is_admin: bool,
}

/// What a successful claim does. The shell applies the plan in one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimPlan {
    /// The claimant already holds the task: only its lease is renewed (version bumps).
    Renew { expires_at: Timestamp },
    /// Take the task. `release_expired` names an expired foreign claimant; the shell first moves
    /// the task to ready with [`REASON_EXPIRED_CLAIM_RELEASED`]. `set_assignee` is true when the
    /// task has no assignee yet.
    Take {
        claimant: String,
        expires_at: Timestamp,
        release_expired: Option<String>,
        set_assignee: bool,
    },
}

/// Checks the optimistic-concurrency version of a write (`_touch`).
pub fn check_version(task: &Task, expected_version: Option<i64>) -> Result<(), WgError> {
    match expected_version {
        Some(expected) if expected != task.version => Err(WgError::new(
            ErrorCode::VersionConflict,
            format!(
                "task {} was modified concurrently (expected version {expected}, found {}); re-read and retry",
                task.key, task.version
            ),
        )
        .with_detail("expected", json!(expected))
        .with_detail("actual", json!(task.version))),
        _ => Ok(()),
    }
}

/// `at` plus `minutes`, rejecting leases outside `1..=MAX_LEASE_MINUTES`.
pub fn lease_expiry(at: Timestamp, minutes: i64) -> Result<Timestamp, WgError> {
    if !(1..=MAX_LEASE_MINUTES).contains(&minutes) {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("lease_minutes must be between 1 and {MAX_LEASE_MINUTES}"),
        ));
    }
    Timestamp::from_offset_datetime(at.to_offset_datetime() + Duration::minutes(minutes))
}

/// The claim decision of `_claim_in_session`.
///
/// `blockers` are the unsatisfied dependencies of the task (from `deps::unsatisfied_dependencies`);
/// a claim with blockers is refused. `agent` is the `ClaimRequest.agent` name; when given the
/// claimant is `agent:<name>`, otherwise the caller's actor.
pub fn plan_claim(
    task: &Task,
    caller: &str,
    agent: Option<&str>,
    lease_minutes: i64,
    expected_version: Option<i64>,
    now: Timestamp,
    blockers: &[&Task],
) -> Result<ClaimPlan, WgError> {
    check_version(task, expected_version)?;
    let claimant = agent.map_or_else(|| caller.to_owned(), |name| format!("agent:{name}"));
    let expires_at = lease_expiry(now, lease_minutes)?;

    let (release_expired, from) =
        if matches!(task.status, TaskStatus::Claimed | TaskStatus::Running)
            && let Some(holder) = task.claimed_by.as_deref()
        {
            let lease_valid = task.claim_expires_at.is_none_or(|expiry| expiry > now);
            if holder != claimant && lease_valid {
                return Err(WgError::new(
                    ErrorCode::ResourceBusy,
                    format!("task {} is already claimed by {holder}", task.key),
                )
                .with_detail("claimed_by", json!(holder))
                // `str(task.claim_expires_at)`: a space between date and time, `None` for no lease.
                .with_detail(
                    "expires_at",
                    json!(task.claim_expires_at.map_or_else(
                        || "None".to_owned(),
                        |expiry| expiry.format_isoformat().replacen('T', " ", 1)
                    )),
                ));
            }
            if holder == claimant {
                return Ok(ClaimPlan::Renew { expires_at });
            }
            // An expired foreign claim is released before the task is re-claimed.
            (Some(holder.to_owned()), TaskStatus::Ready)
        } else if !matches!(task.status, TaskStatus::Ready | TaskStatus::Failed) {
            return Err(WgError::new(
                ErrorCode::InvalidTransition,
                format!(
                    "task {} is {}; only ready (or failed) tasks can be claimed",
                    task.key, task.status
                ),
            )
            .with_detail("status", json!(task.status.as_str())));
        } else {
            (None, task.status)
        };

    if !blockers.is_empty() {
        let keys: Vec<String> = blockers.iter().map(|b| b.key.to_string()).collect();
        return Err(WgError::new(
            ErrorCode::InvalidTransition,
            format!("task {} is blocked by {}", task.key, keys.join(", ")),
        )
        .with_detail("blocked_by", json!(keys)));
    }
    check_task_transition(from, TaskStatus::Claimed, false, false)?;

    Ok(ClaimPlan::Take {
        claimant,
        expires_at,
        release_expired,
        set_assignee: task.assignee.is_none(),
    })
}

/// What a release does: the task goes back to ready and a `task.released` event is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePlan {
    /// The claimant released from (`None` when the task had no claimant).
    pub previous_claimant: Option<String>,
    /// The transition reason: the caller's reason, or [`REASON_RELEASED`].
    pub reason: String,
    /// The event payload's `reason` (the caller's reason as given, possibly absent).
    pub event_reason: Option<String>,
}

/// The release decision of `release`. Only the holder, or an admin, may release a claim.
pub fn plan_release(
    task: &Task,
    caller: Caller<'_>,
    reason: Option<&str>,
) -> Result<ReleasePlan, WgError> {
    if !matches!(task.status, TaskStatus::Claimed | TaskStatus::Running) {
        return Err(WgError::new(
            ErrorCode::InvalidTransition,
            format!("task {} is {}, not claimed", task.key, task.status),
        ));
    }
    if let Some(holder) = task.claimed_by.as_deref()
        && holder != caller.actor
        && !caller.is_admin
    {
        return Err(WgError::new(
            ErrorCode::PermissionDenied,
            format!("task {} is claimed by {holder}", task.key),
        ));
    }
    check_task_transition(task.status, TaskStatus::Ready, false, false)?;
    Ok(ReleasePlan {
        previous_claimant: task.claimed_by.clone(),
        reason: reason
            .filter(|text| !text.is_empty())
            .map_or_else(|| REASON_RELEASED.to_owned(), str::to_owned),
        event_reason: reason.map(str::to_owned),
    })
}

/// The claimed tasks whose lease ended before `now`, by task number (`release_expired_claims`).
/// The shell moves each one to ready with [`REASON_LEASE_EXPIRED`].
pub fn expired_claims(tasks: &TaskSlice, now: Timestamp) -> Vec<&Task> {
    tasks
        .tasks
        .values()
        .filter(|task| {
            task.status == TaskStatus::Claimed
                && task.claim_expires_at.is_some_and(|expiry| expiry < now)
        })
        .map(|task| (task.number, task))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

/// The completion decision of `complete`: agent-declared, never accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletePlan {
    /// `awaiting_verification` when verification checks apply, else `awaiting_review`.
    pub target: TaskStatus,
    /// Whether verification checks apply to the task (the event's `verification_configured`).
    pub verification_configured: bool,
}

/// The completion decision of `_complete_in_session`. `has_checks` is whether any verification
/// check applies to the task (`applicable_checks`).
pub fn plan_complete(
    task: &Task,
    caller: Caller<'_>,
    has_checks: bool,
) -> Result<CompletePlan, WgError> {
    if !matches!(task.status, TaskStatus::Claimed | TaskStatus::Running) {
        return Err(WgError::new(
            ErrorCode::InvalidTransition,
            format!(
                "task {} is {}; claim it before completing",
                task.key, task.status
            ),
        )
        .with_detail("status", json!(task.status.as_str())));
    }
    if let Some(holder) = task.claimed_by.as_deref()
        && holder != caller.actor
        && !caller.actor.starts_with(SYSTEM_ACTOR_PREFIX)
        && !caller.is_admin
    {
        return Err(WgError::new(
            ErrorCode::PermissionDenied,
            format!("task {} is claimed by {holder}", task.key),
        ));
    }
    let target = if has_checks {
        TaskStatus::AwaitingVerification
    } else {
        TaskStatus::AwaitingReview
    };
    check_task_transition(task.status, target, false, false)?;
    Ok(CompletePlan {
        target,
        verification_configured: has_checks,
    })
}

/// The payload of the `task.completed` event (`summary` is cut to 5000 characters).
pub fn complete_event_data(plan: &CompletePlan, summary: &str) -> Value {
    json!({
        "summary": summary.chars().take(5000).collect::<String>(),
        "next_status": plan.target.as_str(),
        "verification_configured": plan.verification_configured,
    })
}

/// A result comment the shell records: the body, and whether it reports a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultComment {
    pub body: String,
    /// True for a failed attempt; the comment then carries `data = {"outcome": "failed"}`.
    pub failed: bool,
}

/// What `report_result` does for an attempt's self-reported outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultPlan {
    /// Set when a success completes claimed or running work. Its result comment is `comment`,
    /// present only when the summary is non-empty.
    pub completion: Option<CompletePlan>,
    /// The result comment to record, when the plan records one.
    pub comment: Option<ResultComment>,
    /// True when a failure moves claimed or running work to failed.
    pub fail: bool,
}

/// The decision of `report_result`. `succeeded` is the attempt outcome; `summary` is the
/// agent's summary (possibly empty). `has_checks` as in [`plan_complete`].
pub fn plan_report_result(
    task: &Task,
    caller: Caller<'_>,
    succeeded: bool,
    summary: &str,
    has_checks: bool,
) -> Result<ResultPlan, WgError> {
    let live = matches!(task.status, TaskStatus::Claimed | TaskStatus::Running);
    if succeeded {
        if live {
            let completion = plan_complete(task, caller, has_checks)?;
            let comment = (!summary.is_empty()).then(|| ResultComment {
                body: summary.to_owned(),
                failed: false,
            });
            return Ok(ResultPlan {
                completion: Some(completion),
                comment,
                fail: false,
            });
        }
        return Ok(ResultPlan {
            completion: None,
            comment: Some(ResultComment {
                body: if summary.is_empty() {
                    "succeeded"
                } else {
                    summary
                }
                .to_owned(),
                failed: false,
            }),
            fail: false,
        });
    }
    Ok(ResultPlan {
        completion: None,
        comment: Some(ResultComment {
            body: if summary.is_empty() {
                "attempt failed"
            } else {
                summary
            }
            .to_owned(),
            failed: true,
        }),
        fail: live,
    })
}
