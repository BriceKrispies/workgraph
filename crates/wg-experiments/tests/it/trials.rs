//! Trial lifecycle guards of `wg_experiments::trials`, checked against the status changes of
//! `ExperimentService` in `src/workgraph/core/services/experiments.py`.

use wg_experiments::trials::{classify, grade, invalidate, start};
use wg_types::{FailureCauseSource, TrialOutcome, TrialStatus};

// covers: C083
#[test]
fn start_moves_only_a_planned_trial_to_running() {
    assert_eq!(start(TrialStatus::Planned), Ok(TrialStatus::Running));
    for status in [
        TrialStatus::Running,
        TrialStatus::Graded,
        TrialStatus::Invalid,
    ] {
        let rejected = start(status).err();
        assert_eq!(rejected.map(|r| r.status), Some(status));
    }
}

// covers: C081
#[test]
fn grade_moves_running_to_graded_and_keeps_invalid() {
    assert_eq!(grade(TrialStatus::Running), Ok(TrialStatus::Graded));
    assert_eq!(grade(TrialStatus::Invalid), Ok(TrialStatus::Invalid));
    for status in [TrialStatus::Planned, TrialStatus::Graded] {
        assert!(grade(status).is_err());
    }
}

// covers: C080
#[test]
fn classify_needs_a_failed_or_invalid_trial() {
    assert!(classify(TrialStatus::Graded, Some(TrialOutcome::Failed)).is_ok());
    assert!(classify(TrialStatus::Invalid, None).is_ok());
    assert!(classify(TrialStatus::Graded, Some(TrialOutcome::Passed)).is_err());
    assert!(classify(TrialStatus::Graded, None).is_err());
    assert!(classify(TrialStatus::Running, None).is_err());
}

#[test]
fn rejection_message_names_the_status_and_the_requirement() {
    let rejected = start(TrialStatus::Running).err();
    let message = rejected.map(|r| r.message());
    assert_eq!(
        message.as_deref(),
        Some("is running; only a planned trial can start")
    );
}

#[test]
fn invalidate_marks_the_trial_invalid_by_a_human() {
    assert_eq!(
        invalidate(),
        (TrialStatus::Invalid, FailureCauseSource::Human)
    );
}
