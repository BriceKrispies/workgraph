//! CORE-11: the external attempt start, submit and abandon checks, the conclusion rules and
//! escalation, checked as pure decisions.

use serde_json::{Map, Value, json};
use wg_exec::attempt::{
    FinishPlan, abandon_plan, duration_ms, escalates, failure_exit_reason, finish_plan, is_success,
    next_attempt_number, sanitize_usage, start_plan, submit_check, task_after_attempt_end,
    task_after_prepare, unsuccessful_status,
};
use wg_types::{AttemptStatus as A, ErrorCode, TaskStatus as S};

#[test]
fn finish_plan_applies_a_running_attempt_and_names_its_event() {
    assert_eq!(
        finish_plan(A::Running, A::Succeeded).ok(),
        Some(FinishPlan::Apply {
            event: "attempt.succeeded"
        })
    );
    assert_eq!(
        finish_plan(A::Running, A::TimedOut).ok(),
        Some(FinishPlan::Apply {
            event: "attempt.failed"
        })
    );
    assert_eq!(
        finish_plan(A::Verifying, A::Cancelled).ok(),
        Some(FinishPlan::Apply {
            event: "attempt.cancelled"
        })
    );
}

#[test]
fn finish_plan_is_a_no_op_for_the_same_terminal_status() {
    assert_eq!(
        finish_plan(A::Failed, A::Failed).ok(),
        Some(FinishPlan::Unchanged)
    );
}

#[test]
fn finish_plan_rejects_a_second_terminal_status_and_a_live_target() {
    let err = finish_plan(A::Succeeded, A::Failed).expect_err("error");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    let err = finish_plan(A::Running, A::Queued).expect_err("error");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    let err = finish_plan(A::Queued, A::Succeeded).expect_err("error");
    assert_eq!(err.code, ErrorCode::InvalidTransition);
}

#[test]
fn next_attempt_number_follows_the_largest_existing_number() {
    assert_eq!(next_attempt_number(&[]), 1);
    assert_eq!(next_attempt_number(&[1, 3, 2]), 4);
}

#[test]
fn duration_prefers_the_reported_value_and_clamps_elapsed_time() {
    assert_eq!(duration_ms(Some(40), Some(900)), Some(40));
    assert_eq!(duration_ms(None, Some(-5)), Some(0));
    assert_eq!(duration_ms(None, None), None);
}

#[test]
fn sanitize_usage_keeps_known_counters_as_whole_non_negative_numbers() {
    let usage: Map<String, Value> = serde_json::from_value(json!({
        "input_tokens": 12,
        "output_tokens": 3.9,
        "total_tokens": -1,
        "num_turns": true,
        "cost": 4,
        "cache_read_input_tokens": "7"
    }))
    .expect("object");
    let clean = sanitize_usage(&usage);
    assert_eq!(
        Value::Object(clean),
        json!({"input_tokens": 12, "output_tokens": 3})
    );
}

#[test]
fn external_start_claims_a_ready_task_and_continues_an_owned_claim() {
    let plan = start_plan("WG-1", S::Ready, false, None, None).expect("ready starts");
    assert!(plan.claim_first);
    let plan = start_plan("WG-1", S::Claimed, true, None, None).expect("own claim continues");
    assert!(!plan.claim_first);
    assert_eq!(task_after_prepare(S::Claimed), S::Running);
    assert_eq!(task_after_prepare(S::Ready), S::Ready);
}

#[test]
fn external_start_names_another_holder_and_refuses_an_active_attempt() {
    let err = start_plan("WG-1", S::Claimed, false, Some("bob"), None).expect_err("held by bob");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(err.message.contains("by bob"), "{}", err.message);

    let err = start_plan("WG-1", S::Ready, false, None, Some(2)).expect_err("active attempt");
    assert_eq!(err.code, ErrorCode::InvalidTransition);
    assert_eq!(err.message, "task WG-1 already has an active attempt #2");
}

#[test]
fn task_after_attempt_end_returns_cancelled_work_to_ready_and_leaves_others_alone() {
    assert_eq!(
        task_after_attempt_end(S::Running, A::Cancelled),
        Some(S::Ready)
    );
    assert_eq!(
        task_after_attempt_end(S::Claimed, A::Failed),
        Some(S::Failed)
    );
    assert_eq!(task_after_attempt_end(S::Accepted, A::Failed), None);
}

#[test]
fn submit_checks_status_attempt_worktree_and_task_in_order() {
    assert!(submit_check("succeeded", A::Running, true, "WG-1", S::Running).is_ok());
    let err =
        submit_check("cancelled", A::Running, true, "WG-1", S::Running).expect_err("bad status");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    let err = submit_check("failed", A::Succeeded, true, "WG-1", S::Running)
        .expect_err("finished attempt");
    assert!(err.message.contains("only a running attempt"));
    let err =
        submit_check("failed", A::Running, false, "WG-1", S::Running).expect_err("no worktree");
    assert!(err.message.contains("no managed worktree"));
    let err =
        submit_check("failed", A::Running, true, "WG-1", S::Ready).expect_err("task not running");
    assert!(err.message.contains("only a running task"));
}

#[test]
fn abandon_needs_a_reason_an_external_attempt_and_a_live_attempt() {
    let err = abandon_plan(true, A::Running, "  ", S::Running).expect_err("reason");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(abandon_plan(false, A::Running, "gone", S::Running).is_err());
    assert!(abandon_plan(true, A::Succeeded, "gone", S::Running).is_err());
    assert_eq!(
        abandon_plan(true, A::Preparing, "gone", S::Claimed).ok(),
        Some(Some(S::Ready))
    );
    assert_eq!(
        abandon_plan(true, A::Running, "gone", S::Failed).ok(),
        Some(None)
    );
}

#[test]
fn conclusion_maps_agent_results_and_overturns_success_on_violations() {
    assert_eq!(unsuccessful_status("succeeded"), Some(A::Failed));
    assert_eq!(unsuccessful_status("timed_out"), Some(A::TimedOut));
    assert_eq!(unsuccessful_status("bogus"), None);
    assert!(is_success("succeeded", false, false));
    assert!(!is_success("succeeded", true, false));
    assert!(!is_success("succeeded", false, true));
    assert_eq!(failure_exit_reason("failed", false), "failed");
    assert_eq!(
        failure_exit_reason("succeeded", true),
        "outcome_capture_failed"
    );
    assert_eq!(failure_exit_reason("succeeded", false), "policy_violation");
}

#[test]
fn escalation_needs_a_failed_task_at_the_limit_without_the_flag() {
    assert!(escalates(S::Failed, 3, 3, false));
    assert!(!escalates(S::Failed, 2, 3, false));
    assert!(!escalates(S::Failed, 3, 3, true));
    assert!(!escalates(S::Ready, 3, 3, false));
}
