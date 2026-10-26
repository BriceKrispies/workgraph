//! CORE-01: the lifecycle cases of `tests/unit/test_tasks_core.py`, ported to the transition checks.
//! The Python test drives the service; here the same transitions are checked against the pure
//! lifecycle module.

use wg_tasks::lifecycle::check_task_transition;
use wg_types::{ErrorCode, TaskStatus};

/// `test_status_transitions_are_validated`: a ready task cannot be accepted or sent to review
/// by a plain update, and can be cancelled.
#[test]
fn status_transitions_are_validated() {
    let ready = TaskStatus::Ready;

    let accepted = check_task_transition(ready, TaskStatus::Accepted, false, false);
    let err = accepted
        .err()
        .unwrap_or_else(|| panic!("accepting a ready task must fail"));
    assert_eq!(err.code, ErrorCode::InvalidTransition);

    let review = check_task_transition(ready, TaskStatus::AwaitingReview, false, false);
    let err = review
        .err()
        .unwrap_or_else(|| panic!("ready to awaiting_review must fail"));
    assert_eq!(err.code, ErrorCode::InvalidTransition);

    assert!(check_task_transition(ready, TaskStatus::Cancelled, false, false).is_ok());
}
