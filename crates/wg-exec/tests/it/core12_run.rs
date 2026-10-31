//! CORE-12: the managed run flow as pure decisions, covering the cases of `test_runs.py`.

use std::path::{Path, PathBuf};

use wg_exec::run::{
    BaselineEntry, CancelPlan, CaptureFacts, Checkout, Conclusion, EarlyEnd, ExecuteEntry, Gate,
    PostSuccess, RunStage, VerifyStep, adapter_crash_result, agent_env, cancel_plan,
    choose_workdir, conclude, events_to_write, execute_entry, flush_due, kept_summary,
    managed_start_plan, marks_ready, post_success, records_event, require_execution, rerun_plan,
    resource_keys, retry_check, verification_error_end, verify_step,
};
use wg_types::{AttemptStatus as A, ErrorCode, TaskStatus as S};

#[test]
fn execution_is_opt_in() {
    let err = require_execution(false).unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(require_execution(true).is_ok());
}

#[test]
fn start_claims_ready_and_failed_tasks_and_refuses_active_attempts() {
    assert!(
        managed_start_plan("T-1", S::Ready, None, false)
            .unwrap()
            .claim_first
    );
    assert!(
        managed_start_plan("T-1", S::Failed, None, false)
            .unwrap()
            .claim_first
    );
    assert!(
        !managed_start_plan("T-1", S::Claimed, None, false)
            .unwrap()
            .claim_first
    );
    assert!(managed_start_plan("T-1", S::Accepted, None, false).is_err());
    let busy = managed_start_plan("T-1", S::Claimed, Some(2), false).unwrap_err();
    assert_eq!(busy.code, ErrorCode::InvalidTransition);
    assert!(managed_start_plan("T-1", S::Claimed, Some(2), true).is_ok());
}

#[test]
fn in_place_runs_lock_the_task_repositories() {
    assert_eq!(
        resource_keys("codex", "worktree", "t"),
        ["run", "agent:codex"]
    );
    assert_eq!(
        resource_keys("codex", "in_place", "t"),
        ["run", "agent:codex", "exclusive:task-repos:t"]
    );
}

#[test]
fn stages_walk_forward_to_conclusion() {
    let mut stage = RunStage::Queued;
    let mut seen = vec![stage];
    while let Some(next) = stage.next() {
        assert!(next > stage);
        stage = next;
        seen.push(stage);
    }
    assert_eq!(seen.len(), 7);
    assert_eq!(RunStage::Verify.attempt_status(), Some(A::Verifying));
    assert_eq!(RunStage::Concluded.attempt_status(), None);
}

#[test]
fn execute_skips_finished_attempts_and_honours_early_cancel() {
    assert_eq!(
        execute_entry(A::Failed, false),
        ExecuteEntry::Skip("attempt already failed".into())
    );
    assert_eq!(
        execute_entry(A::Queued, true),
        ExecuteEntry::CancelBeforeStart
    );
    assert_eq!(execute_entry(A::Queued, false), ExecuteEntry::Proceed);
    let end = EarlyEnd::cancelled_before_start();
    assert_eq!(end.task_after(S::Running), Some(S::Ready));
}

#[test]
fn unavailable_provider_and_preparation_failure_fail_the_task() {
    let end = EarlyEnd::provider_unavailable("no claude");
    assert_eq!(end.exit_reason, "provider_unavailable");
    assert_eq!(end.task_after(S::Claimed), Some(S::Failed));
    assert_eq!(end.task_after(S::Accepted), None);
    let prep = EarlyEnd::preparation_failed("bad repo");
    assert_eq!(prep.error, "workspace preparation failed: bad repo");
}

#[test]
fn adapter_crash_is_preserved_as_a_failed_result() {
    let r = adapter_crash_result("RuntimeError", "boom");
    assert_eq!(r.status, "failed");
    assert_eq!(r.exit_reason, "adapter_crash");
    assert_eq!(r.error, "adapter crashed: RuntimeError: boom");
}

#[test]
fn conclusion_reports_success_failure_timeout_and_cancel() {
    let clean = CaptureFacts::default();
    assert_eq!(
        conclude("succeeded", None, &clean).unwrap(),
        Conclusion::Succeeded
    );
    let failed = conclude("failed", Some("nope"), &clean).unwrap();
    assert_eq!(
        failed,
        Conclusion::Failed {
            attempt: A::Failed,
            exit_reason: "failed".into(),
            error: "nope".into()
        }
    );
    let Conclusion::Failed { attempt, error, .. } = conclude("timed_out", None, &clean).unwrap()
    else {
        panic!("timeout is a failure");
    };
    assert_eq!((attempt, error.as_str()), (A::TimedOut, "agent failed"));
    let Conclusion::Failed { attempt, .. } = conclude("cancelled", None, &clean).unwrap() else {
        panic!("cancel is a failure");
    };
    assert_eq!(attempt, A::Cancelled);
    assert!(conclude("weird", None, &clean).is_err());
}

#[test]
fn read_only_violation_overturns_a_reported_success() {
    let facts = CaptureFacts {
        violations: Some("['docs']".into()),
        capture_error: None,
    };
    let Conclusion::Failed {
        attempt,
        exit_reason,
        error,
    } = conclude("succeeded", Some("done"), &facts).unwrap()
    else {
        panic!("violations fail the attempt");
    };
    assert_eq!(attempt, A::Failed);
    assert_eq!(exit_reason, "policy_violation");
    assert_eq!(
        error,
        "read-only repositories were modified: ['docs']. done"
    );
}

#[test]
fn capture_failure_overturns_a_reported_success() {
    let facts = CaptureFacts {
        violations: None,
        capture_error: Some("git failed".into()),
    };
    let Conclusion::Failed {
        exit_reason, error, ..
    } = conclude("succeeded", None, &facts).unwrap()
    else {
        panic!("capture failure fails the attempt");
    };
    assert_eq!(exit_reason, "outcome_capture_failed");
    assert_eq!(error, "could not record the attempt's changes: git failed.");
}

#[test]
fn gates_pick_the_verification_step() {
    assert_eq!(Gate::parse(None), Some(Gate::NoStack));
    assert_eq!(Gate::parse(Some("odd")), None);
    assert_eq!(verify_step(Gate::Pass), VerifyStep::SkipMarkReady);
    assert_eq!(verify_step(Gate::Fast), VerifyStep::Run { fast_only: true });
    assert_eq!(
        verify_step(Gate::NoStack),
        VerifyStep::Run { fast_only: false }
    );
    assert!(marks_ready(Gate::Fast, "passed"));
    assert!(!marks_ready(Gate::Fast, "failed"));
    assert!(!marks_ready(Gate::NoStack, "passed"));
    assert_eq!(post_success(S::AwaitingVerification), PostSuccess::Verify);
    assert_eq!(post_success(S::Accepted), PostSuccess::SucceedNow);
}

#[test]
fn verification_error_fails_only_an_unfinished_attempt() {
    let end = verification_error_end(A::Verifying, "boom").unwrap();
    assert_eq!(end.exit_reason, "verification_error");
    assert!(verification_error_end(A::Succeeded, "boom").is_none());
}

#[test]
fn cancel_plans_follow_attempt_and_job_state() {
    assert_eq!(
        cancel_plan(A::Succeeded, Some("done")),
        CancelPlan::Unchanged
    );
    assert_eq!(
        cancel_plan(A::Queued, Some("queued")),
        CancelPlan::CancelQueued { cancel_job: true }
    );
    assert_eq!(
        cancel_plan(A::Queued, Some("running")),
        CancelPlan::CancelQueued { cancel_job: false }
    );
    assert_eq!(
        cancel_plan(A::Running, Some("running")),
        CancelPlan::RequestStop { flag_job: true }
    );
    assert_eq!(
        cancel_plan(A::Running, None),
        CancelPlan::RequestStop { flag_job: false }
    );
}

#[test]
fn retry_needs_a_finished_unsuccessful_attempt() {
    assert!(retry_check(A::Failed).is_ok());
    assert!(retry_check(A::Running).is_err());
    assert!(retry_check(A::Succeeded).is_err());
}

#[test]
fn rerun_pins_available_revisions_and_reports_missing_ones() {
    let entry = |id: &str, sha: Option<&str>, ok: bool| BaselineEntry {
        repository_id: id.into(),
        repository: id.into(),
        access: "read_write".into(),
        sha: sha.map(Into::into),
        revision_available: ok,
    };
    let plan = rerun_plan(&[
        entry("a", Some("abc"), true),
        entry("b", Some("def"), false),
    ]);
    assert!(!plan.reproduced_baseline());
    assert_eq!(plan.repositories[0].target_ref.as_deref(), Some("abc"));
    assert_eq!(plan.repositories[1].target_ref, None);
    assert_eq!(plan.missing[0].repository, "b");
    assert!(rerun_plan(&[entry("a", Some("abc"), true)]).reproduced_baseline());
}

#[test]
fn workdir_is_the_first_writable_checkout() {
    let c = |p: &str, rw| Checkout {
        path: PathBuf::from(p),
        read_write: rw,
    };
    let chosen = choose_workdir(&[c("/r/ro", false), c("/r/rw", true)], Path::new("/ws"));
    assert_eq!(chosen.workdir, PathBuf::from("/r/rw"));
    assert_eq!(chosen.additional_dirs, [PathBuf::from("/r/ro")]);
    let none = choose_workdir(&[c("/r/ro", false)], Path::new("/ws"));
    assert_eq!(none.workdir, PathBuf::from("/ws"));
}

#[test]
fn agent_env_drops_empty_values() {
    let env = agent_env("a", "t", "T-1", None, "/m.json");
    assert!(env.iter().all(|(k, _)| k != "WORKGRAPH_HOME"));
    assert_eq!(env.len(), 4);
}

#[test]
fn event_recording_is_throttled_and_capped() {
    assert!(records_event("progress", 10_000));
    assert!(!records_event("log", 501));
    assert!(records_event("log", 500));
    assert!(flush_due("result", 0.0));
    assert!(!flush_due("log", 0.5));
    assert!(flush_due("log", 1.5));
    assert_eq!(events_to_write(30, 0), 20);
    assert_eq!(events_to_write(3, 499), 3);
    assert_eq!(events_to_write(3, 500), 0);
}

#[test]
fn summary_is_capped_and_falls_back_to_the_old_one() {
    let long = "x".repeat(60_000);
    assert_eq!(kept_summary(Some(&long), None).unwrap().len(), 50_000);
    assert_eq!(kept_summary(Some(""), Some("old")).as_deref(), Some("old"));
}
