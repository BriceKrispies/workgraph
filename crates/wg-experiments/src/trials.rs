//! Trial lifecycle decisions: the status changes behind `start`, `grade`, `classify` and
//! `mark_invalid` in `src/workgraph/core/services/experiments.py` (CORE-25).
//!
//! The service reads and writes the trial row and records the event; these functions decide only
//! whether a status change is allowed and what the new status is.

use wg_types::{FailureCauseSource, TrialOutcome, TrialStatus};

/// A refused status change: the trial's current status and what the operation requires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rejected {
    /// The status the trial has.
    pub status: TrialStatus,
    /// What the operation requires, as the Python message words it.
    pub requirement: &'static str,
}

impl Rejected {
    /// The Python `ConflictError` message, without the trial id the shell prefixes.
    #[must_use]
    pub fn message(&self) -> String {
        format!("is {}; {}", self.status.as_str(), self.requirement)
    }
}

/// `start_trial`: only a planned trial starts, and it becomes running.
///
/// # Errors
///
/// Returns [`Rejected`] when the trial is not planned.
pub fn start(status: TrialStatus) -> Result<TrialStatus, Rejected> {
    match status {
        TrialStatus::Planned => Ok(TrialStatus::Running),
        other => Err(Rejected {
            status: other,
            requirement: "only a planned trial can start",
        }),
    }
}

/// `record_grade`: a running trial becomes graded; an invalid trial keeps its status and still
/// stores its outcome.
///
/// # Errors
///
/// Returns [`Rejected`] when the trial is neither running nor invalid.
pub fn grade(status: TrialStatus) -> Result<TrialStatus, Rejected> {
    match status {
        TrialStatus::Running => Ok(TrialStatus::Graded),
        TrialStatus::Invalid => Ok(TrialStatus::Invalid),
        other => Err(Rejected {
            status: other,
            requirement: "only a running or invalid trial takes a grade",
        }),
    }
}

/// `classify`: only a failed or invalid trial takes a failure cause.
///
/// `outcome` is the recorded outcome, if any; a graded trial counts as failed only when its
/// outcome is [`TrialOutcome::Failed`].
///
/// # Errors
///
/// Returns [`Rejected`] when the trial is neither a failed graded trial nor invalid.
pub fn classify(status: TrialStatus, outcome: Option<TrialOutcome>) -> Result<(), Rejected> {
    let failed = status == TrialStatus::Graded && outcome == Some(TrialOutcome::Failed);
    if failed || status == TrialStatus::Invalid {
        Ok(())
    } else {
        Err(Rejected {
            status,
            requirement: "only failed or invalid trials are classified",
        })
    }
}

/// `mark_invalid`: any trial becomes invalid, excluded from every fit, and the cause is recorded
/// as a human decision.
#[must_use]
pub fn invalidate() -> (TrialStatus, FailureCauseSource) {
    (TrialStatus::Invalid, FailureCauseSource::Human)
}
