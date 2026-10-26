//! Run outcome: how one check's process result becomes a status, how a run's check statuses become
//! the run's status and `required_passed`, and when a run covers the full tier. Port of
//! `_execute_check` (status and signatures), `_finalize` (run status) and `_ran_fast_tier_only`
//! in `verification/service.py`.
//!
//! The shell runs the process and passes the facts in as [`ProcessOutcome`]; it also reads the
//! log, since the signatures are extracted from the whole log when it can be read.

use wg_model::entity::JsonObject;
use wg_types::VerificationStatus;

use crate::config::{CheckSpec, CheckTier};
use crate::scope::ApplicableCheck;
use crate::signatures::{FailureParser, extract_failure_signatures};

/// The facts of one finished check process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutcome {
    pub ok: bool,
    pub cancelled: bool,
    pub timed_out: bool,
    pub exit_code: Option<i64>,
}

/// The status of one check. A cancelled run, or an exit code the check lists as "could not judge",
/// is an error and never a pass or a regression.
pub fn check_status(spec: &CheckSpec, outcome: &ProcessOutcome) -> VerificationStatus {
    if outcome.ok {
        VerificationStatus::Passed
    } else if outcome.cancelled
        || outcome
            .exit_code
            .is_some_and(|code| spec.error_exit_codes.contains(&code))
    {
        VerificationStatus::Error
    } else {
        VerificationStatus::Failed
    }
}

/// The failure signatures of one check. A passing check has none; a timeout is always the first.
pub fn check_signatures<S: AsRef<str>>(
    ok: bool,
    timed_out: bool,
    log_lines: &[S],
    parser: FailureParser,
) -> Vec<String> {
    let signatures = if ok {
        Vec::new()
    } else {
        extract_failure_signatures(log_lines, parser)
    };
    if timed_out {
        std::iter::once("timeout".to_string())
            .chain(signatures)
            .collect()
    } else {
        signatures
    }
}

/// One finished check of a run, as the outcome needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: String,
    pub required: bool,
    pub status: VerificationStatus,
}

/// The outcome of a whole run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// `Passed` when every required check passed, otherwise `Failed`.
    pub status: VerificationStatus,
    pub required_passed: bool,
    /// Every check that did not pass, required or not, in order.
    pub failed_checks: Vec<String>,
    /// Some optional check failed while the required ones passed.
    pub optional_failures: bool,
}

/// Combines the check results of a run. Only required checks decide the run.
pub fn run_outcome(results: &[CheckResult]) -> RunOutcome {
    let failing: Vec<&CheckResult> = results
        .iter()
        .filter(|result| result.status != VerificationStatus::Passed)
        .collect();
    let any_failed = !failing.is_empty();
    let required_passed = !failing.iter().any(|result| result.required);
    let failed_checks = failing.iter().map(|result| result.name.clone()).collect();
    RunOutcome {
        status: if required_passed {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Failed
        },
        required_passed,
        failed_checks,
        optional_failures: any_failed && required_passed,
    }
}

/// Whether a fast-tier pass gates a stack child only. Such a pass does not move the change set or
/// the task; the full tier on the integration tip does.
pub fn is_fast_pass(tier: Option<CheckTier>, required_passed: bool) -> bool {
    tier == Some(CheckTier::Fast) && required_passed
}

/// Whether a run covered only the fast tier of a configuration that has full checks. Such a run
/// cannot accept a task. A run whose snapshot lacks a `tier` key counts as full, as the stored
/// configs before tiers existed did.
pub fn ran_fast_tier_only(applicable: &[ApplicableCheck], config_snapshot: &[JsonObject]) -> bool {
    if !applicable
        .iter()
        .any(|check| check.spec.tier == CheckTier::Full)
    {
        return false;
    }
    !config_snapshot.iter().any(|entry| match entry.get("tier") {
        None => true,
        Some(value) => value.as_str() == Some(CheckTier::Full.as_str()),
    })
}
