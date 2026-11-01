//! Run outcome, fast-tier coverage, and the acceptance guards.
#![allow(clippy::expect_used)]

use serde_json::{Value, json};
use wg_model::entity::JsonObject;
use wg_types::{Id, VerificationStatus};
use wg_verify::{
    AcceptRefusal, ApplicableCheck, AutoAcceptFacts, AutoAcceptSkip, CheckResult, CheckSpec,
    CheckTier, LatestRun, ProcessOutcome, accept_refusal, auto_accept_skip, check_status,
    is_fast_pass, ran_fast_tier_only, run_outcome,
};

fn check(name: &str, tier: &str) -> ApplicableCheck {
    let raw = json!({"name": name, "command": ["x"], "tier": tier});
    ApplicableCheck {
        config_id: Id::nil(),
        config_name: "ci".into(),
        scope_type: "workspace".into(),
        scope_id: Id::nil(),
        spec: CheckSpec::from_json(raw.as_object().expect("object")).expect("valid"),
    }
}

fn result(name: &str, required: bool, status: VerificationStatus) -> CheckResult {
    CheckResult {
        name: name.into(),
        required,
        status,
    }
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => JsonObject::new(),
    }
}

fn snapshot(tiers: &[Option<&str>]) -> Vec<JsonObject> {
    tiers
        .iter()
        .map(|tier| match tier {
            Some(t) => object(json!({"tier": t})),
            None => object(json!({"name": "no-tier-key"})),
        })
        .collect()
}

fn facts() -> AutoAcceptFacts {
    AutoAcceptFacts {
        holds_task_accept: true,
        auto_accept_enabled: true,
        any_required_check: true,
        ran_fast_tier_only: false,
        tracked: false,
        awaiting_review: true,
        active_stack_item: false,
        active_stack_parent: false,
        lands_through_stacks: false,
        unaccepted_blockers: false,
        is_latest_run: true,
    }
}

#[test]
fn a_failed_check_is_error_when_cancelled_or_listed_as_unjudged() {
    let spec = CheckSpec::from_json(
        json!({"name": "s", "command": ["x"], "error_exit_codes": [75]})
            .as_object()
            .expect("object"),
    )
    .expect("valid");
    let exited = |code| ProcessOutcome {
        ok: false,
        cancelled: false,
        timed_out: false,
        exit_code: Some(code),
    };
    assert_eq!(check_status(&spec, &exited(75)), VerificationStatus::Error);
    assert_eq!(check_status(&spec, &exited(1)), VerificationStatus::Failed);
    let cancelled = ProcessOutcome {
        ok: false,
        cancelled: true,
        timed_out: false,
        exit_code: None,
    };
    assert_eq!(check_status(&spec, &cancelled), VerificationStatus::Error);
}

#[test]
fn only_required_failures_fail_the_run() {
    let optional_only = run_outcome(&[
        result("unit", true, VerificationStatus::Passed),
        result("lint", false, VerificationStatus::Failed),
    ]);
    assert_eq!(optional_only.status, VerificationStatus::Passed);
    assert!(optional_only.required_passed);
    assert!(optional_only.optional_failures);
    assert_eq!(optional_only.failed_checks, ["lint"]);

    let required_failed = run_outcome(&[result("unit", true, VerificationStatus::Error)]);
    assert_eq!(required_failed.status, VerificationStatus::Failed);
    assert!(!required_failed.required_passed);
    assert!(!required_failed.optional_failures);
}

#[test]
fn fast_pass_gates_only_a_fast_run_that_passed() {
    assert!(is_fast_pass(Some(CheckTier::Fast), true));
    assert!(!is_fast_pass(Some(CheckTier::Fast), false));
    assert!(!is_fast_pass(Some(CheckTier::Full), true));
    assert!(!is_fast_pass(None, true));
}

#[test]
fn a_fast_only_snapshot_cannot_accept_when_full_checks_exist() {
    let mixed = [check("lint", "fast"), check("unit", "full")];
    assert!(ran_fast_tier_only(&mixed, &snapshot(&[Some("fast")])));
    assert!(!ran_fast_tier_only(
        &mixed,
        &snapshot(&[Some("fast"), Some("full")])
    ));
    // A snapshot entry without a tier key counts as full, as stored configs before tiers did.
    assert!(!ran_fast_tier_only(&mixed, &snapshot(&[None])));
    let fast_only = [check("lint", "fast")];
    assert!(!ran_fast_tier_only(&fast_only, &snapshot(&[Some("fast")])));
}

#[test]
fn auto_accept_guards_run_in_order() {
    assert_eq!(auto_accept_skip(&facts()), None);
    let cases = [
        (
            AutoAcceptFacts {
                holds_task_accept: false,
                ..facts()
            },
            AutoAcceptSkip::NoPermission,
        ),
        (
            AutoAcceptFacts {
                auto_accept_enabled: false,
                ..facts()
            },
            AutoAcceptSkip::Disabled,
        ),
        (
            AutoAcceptFacts {
                tracked: true,
                awaiting_review: false,
                ..facts()
            },
            AutoAcceptSkip::Tracked,
        ),
        (
            AutoAcceptFacts {
                awaiting_review: false,
                ..facts()
            },
            AutoAcceptSkip::NotAwaitingReview,
        ),
        (
            AutoAcceptFacts {
                unaccepted_blockers: true,
                ..facts()
            },
            AutoAcceptSkip::UnacceptedBlockers,
        ),
        (
            AutoAcceptFacts {
                unaccepted_blockers: true,
                is_latest_run: false,
                ..facts()
            },
            AutoAcceptSkip::UnacceptedBlockers,
        ),
        (
            AutoAcceptFacts {
                is_latest_run: false,
                ..facts()
            },
            AutoAcceptSkip::NotLatestRun,
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(auto_accept_skip(&input), Some(expected));
    }
}

#[test]
fn accept_refuses_with_python_messages() {
    let run_id = Id::from_u128(7);
    assert_eq!(
        accept_refusal(0, None),
        Some(AcceptRefusal::NoVerificationConfigured)
    );
    assert_eq!(accept_refusal(2, None), Some(AcceptRefusal::NoCompletedRun));
    let failed = LatestRun {
        run_id,
        status: VerificationStatus::Failed,
        required_passed: false,
        fast_tier_only: false,
    };
    assert_eq!(
        accept_refusal(2, Some(failed)),
        Some(AcceptRefusal::LatestRunFailed { run_id })
    );
    let fast = LatestRun {
        required_passed: true,
        status: VerificationStatus::Passed,
        fast_tier_only: true,
        ..failed
    };
    assert_eq!(
        accept_refusal(2, Some(fast)),
        Some(AcceptRefusal::LatestRunFastOnly { run_id })
    );
    let error = AcceptRefusal::LatestRunFastOnly { run_id }.to_error("WG-1");
    assert_eq!(
        error.message,
        "latest verification run for WG-1 ran only fast checks; run full verification first"
    );
}
