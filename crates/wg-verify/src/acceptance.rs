//! Acceptance: the guards of automated acceptance and of the `accept` command, and the choice of
//! a task's latest verification run. Port of `_auto_accept`, `accept` and `_latest_run` in
//! `verification/service.py`.
//!
//! The shell gathers each fact from the slice (or from the stack and tracker queries) and passes
//! it in; the functions here say whether acceptance may proceed and, when it may not, why. Writing
//! the acceptance row is the shell's job.

use wg_model::entity::VerificationRun;
use wg_types::{Id, VerificationStatus, WgError};

/// Facts for the automatic acceptance that follows a finished run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoAcceptFacts {
    /// The actor that finished the run holds `task:accept`.
    pub holds_task_accept: bool,
    /// `verification.auto_accept` is on in the effective configuration.
    pub auto_accept_enabled: bool,
    /// At least one check of the run is required.
    pub any_required_check: bool,
    /// The run covered only the fast tier of a configuration that has full checks.
    pub ran_fast_tier_only: bool,
    /// The task is tracked by an external tracker, which closes it.
    pub tracked: bool,
    /// The task is `awaiting_review`.
    pub awaiting_review: bool,
    /// The task is an active stack item.
    pub active_stack_item: bool,
    /// The task is the parent of an active stack, which lands the stack instead.
    pub active_stack_parent: bool,
    /// A repository in the manifest lands its work through stacks.
    pub lands_through_stacks: bool,
    /// A direct dependency of the task is not accepted yet.
    pub unaccepted_blockers: bool,
    /// The finished run is the task's latest run.
    pub is_latest_run: bool,
}

/// Why automatic acceptance does not happen, in the order the guards are tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoAcceptSkip {
    NoPermission,
    Disabled,
    NoRequiredChecks,
    FastTierOnly,
    Tracked,
    NotAwaitingReview,
    ActiveStackItem,
    StackParent,
    LandsThroughStacks,
    UnacceptedBlockers,
    NotLatestRun,
}

/// Decides automatic acceptance. `None` means the task is accepted.
pub fn auto_accept_skip(facts: &AutoAcceptFacts) -> Option<AutoAcceptSkip> {
    if !facts.holds_task_accept {
        Some(AutoAcceptSkip::NoPermission)
    } else if !facts.auto_accept_enabled {
        Some(AutoAcceptSkip::Disabled)
    } else if !facts.any_required_check {
        Some(AutoAcceptSkip::NoRequiredChecks)
    } else if facts.ran_fast_tier_only {
        Some(AutoAcceptSkip::FastTierOnly)
    } else if facts.tracked {
        Some(AutoAcceptSkip::Tracked)
    } else if !facts.awaiting_review {
        Some(AutoAcceptSkip::NotAwaitingReview)
    } else if facts.active_stack_item {
        Some(AutoAcceptSkip::ActiveStackItem)
    } else if facts.active_stack_parent {
        Some(AutoAcceptSkip::StackParent)
    } else if facts.lands_through_stacks {
        Some(AutoAcceptSkip::LandsThroughStacks)
    } else if facts.unaccepted_blockers {
        Some(AutoAcceptSkip::UnacceptedBlockers)
    } else if !facts.is_latest_run {
        Some(AutoAcceptSkip::NotLatestRun)
    } else {
        None
    }
}

/// The state of a task's latest run, as the `accept` command reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatestRun {
    pub run_id: Id,
    pub status: VerificationStatus,
    pub required_passed: bool,
    pub fast_tier_only: bool,
}

/// Why the `accept` command refuses a task. The variants carry what the message needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptRefusal {
    NoVerificationConfigured,
    NoCompletedRun,
    LatestRunFailed { run_id: Id },
    LatestRunFastOnly { run_id: Id },
}

impl AcceptRefusal {
    /// The domain error for this refusal, with the Python message text.
    pub fn to_error(self, task_key: &str) -> WgError {
        match self {
            Self::NoVerificationConfigured => WgError::validation(format!(
                    "{task_key} has no verification configured; use manual acceptance with a recorded reason"
                )),
            Self::NoCompletedRun => WgError::conflict(format!("{task_key} has no completed verification run")),
            Self::LatestRunFailed { run_id } => WgError::conflict(format!(
                    "latest verification run for {task_key} failed required checks; it cannot be accepted"
                ))
            .detail("run_id", run_id.to_string()),
            Self::LatestRunFastOnly { run_id } => WgError::conflict(format!(
                    "latest verification run for {task_key} ran only fast checks; run full verification first"
                ))
            .detail("run_id", run_id.to_string()),
        }
    }
}

/// Decides whether the `accept` command may accept a task. `applicable_checks` is the number of
/// checks applicable to the task now; `latest` is its latest run, if any.
pub fn accept_refusal(
    applicable_checks: usize,
    latest: Option<LatestRun>,
) -> Option<AcceptRefusal> {
    if applicable_checks == 0 {
        return Some(AcceptRefusal::NoVerificationConfigured);
    }
    match latest {
        None => Some(AcceptRefusal::NoCompletedRun),
        Some(run) if run.status == VerificationStatus::Running => {
            Some(AcceptRefusal::NoCompletedRun)
        }
        Some(run) if !run.required_passed => {
            Some(AcceptRefusal::LatestRunFailed { run_id: run.run_id })
        }
        Some(run) if run.fast_tier_only => {
            Some(AcceptRefusal::LatestRunFastOnly { run_id: run.run_id })
        }
        Some(_) => None,
    }
}

/// The most recently created run of a task. Ties on the creation time break by id, so the choice
/// is the same for every caller.
pub fn latest_run(runs: &[VerificationRun]) -> Option<&VerificationRun> {
    runs.iter()
        .max_by(|a, b| (a.created_at, a.id).cmp(&(b.created_at, b.id)))
}
