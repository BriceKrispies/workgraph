//! WG-4067: `adopt` accepts ready, claimed and awaiting_verification tasks with no active attempt,
//! and refuses the rest.
//!
//! WG-4072: an adopted ref goes through the submit gate. These pin the gate decisions adopt
//! applies: a stack child's fast pass marks it ready, a fast failure does not, a child without
//! fast checks is marked ready without a run, and a task outside a stack runs the full tier.

use wg_exec::attempt::adopt_check;
use wg_exec::run::{Gate, VerifyStep, marks_ready, verify_step};
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

#[test]
fn adopt_fast_pass_marks_a_stack_child_ready() {
    assert_eq!(verify_step(Gate::Fast), VerifyStep::Run { fast_only: true });
    assert!(marks_ready(Gate::Fast, "passed"));
}

#[test]
fn adopt_fast_failure_does_not_mark_a_stack_child_ready() {
    assert!(!marks_ready(Gate::Fast, "failed"));
    assert!(!marks_ready(Gate::Fast, "running"));
}

#[test]
fn adopt_without_fast_checks_is_marked_ready_without_a_run() {
    assert_eq!(verify_step(Gate::Pass), VerifyStep::SkipMarkReady);
}

#[test]
fn adopt_outside_a_stack_runs_the_full_tier() {
    assert_eq!(
        verify_step(Gate::NoStack),
        VerifyStep::Run { fast_only: false }
    );
    assert!(!marks_ready(Gate::NoStack, "passed"));
}
