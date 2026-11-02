//! WG-4067: `adopt` accepts ready, claimed and awaiting_verification tasks with no active attempt,
//! and refuses the rest.

use wg_exec::attempt::adopt_check;
use wg_types::TaskStatus as S;

#[test]
fn adopt_accepts_ready_claimed_and_awaiting_verification() {
    for status in [S::Ready, S::Claimed, S::AwaitingVerification] {
        assert!(
            adopt_check("WG-1", status, false).is_ok(),
            "{status} should adopt"
        );
    }
}

#[test]
fn adopt_refuses_other_statuses() {
    for status in [S::Backlog, S::Running, S::Accepted, S::Failed, S::Cancelled] {
        let err = adopt_check("WG-1", status, false).expect_err("refused");
        assert!(
            err.message.contains("WG-1"),
            "names the task: {}",
            err.message
        );
    }
}

#[test]
fn adopt_refuses_a_task_with_an_active_attempt() {
    let err = adopt_check("WG-1", S::Ready, true).expect_err("refused");
    assert!(err.message.contains("active attempt"), "{}", err.message);
}
