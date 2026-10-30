//! CORE-14: the item, membership, readmit and ejection cases of `tests/integration/test_stacks.py`,
//! ported to the pure decisions of `wg-stacks`. The Python tests drive the service against a real
//! database; here the same transitions, refusals and records are checked against the pure functions.

use serde_json::json;
use uuid::Uuid;
use wg_stacks::ejection::{
    bisected_comment, bisected_ejection, check_ejectable, ejected_detail, manual_comment,
    manual_ejection, parked_detail, returns_to_ready, should_park_dependent,
};
use wg_stacks::item::{
    check_item_transition, is_removable, merge_order_after, satisfies_stack_dependency,
};
use wg_stacks::membership::{
    check_add_item, check_readmit, check_remove_item, outside_repositories, readmit_task_path,
};
use wg_types::{ErrorCode, StackItemState as S, TaskStatus};

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

/// `test_item_transitions_follow_the_table`: the allowed walk, the refused jump, and the no-op.
#[test]
fn item_transitions_follow_the_table() {
    assert!(check_item_transition(S::Pending, S::Ready).is_ok());
    assert!(check_item_transition(S::Ready, S::Merged).is_ok());
    assert!(check_item_transition(S::Merged, S::Conflict).is_ok());
    assert!(check_item_transition(S::Conflict, S::Ready).is_ok());

    let err = check_item_transition(S::Conflict, S::Verified)
        .err()
        .unwrap_or_else(|| panic!("conflict to verified must be refused"));
    assert_eq!(err.code, ErrorCode::InvalidTransition);
    assert_eq!(
        err.details["allowed"],
        json!(["ejected", "pending", "ready"])
    );

    // Unchanged state is a no-op.
    assert!(check_item_transition(S::Merged, S::Merged).is_ok());
}

/// `test_item_transitions_follow_the_table`: merge order is recorded the first time an item merges
/// and survives later re-merges.
#[test]
fn merge_order_is_recorded_once() {
    let first = merge_order_after(None, S::Merged, None);
    assert_eq!(first, Some(1));
    // A second item merging after it takes the next position.
    assert_eq!(merge_order_after(None, S::Merged, Some(1)), Some(2));
    // Re-merging keeps the recorded order.
    assert_eq!(merge_order_after(first, S::Merged, Some(2)), Some(1));
    // Other transitions leave it alone.
    assert_eq!(merge_order_after(None, S::Ready, Some(3)), None);
}

/// `test_add_and_remove_children`: a merged child cannot leave; pending, ready, conflicting and
/// ejected children can.
#[test]
fn remove_refused_only_on_the_integration_branch() {
    let err = check_remove_item("WG-1", S::Merged)
        .err()
        .unwrap_or_else(|| panic!("removing a merged child must fail"));
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(err.details["state"], json!("merged"));

    for state in [S::Pending, S::Ready, S::Conflict, S::Ejected] {
        assert!(is_removable(state));
        assert!(check_remove_item("WG-2", state).is_ok());
    }
    assert!(!is_removable(S::Verified));
}

/// `test_add_and_remove_children`: adding refuses a task that is not a child, is already a member,
/// or links repositories the stack does not cover.
#[test]
fn add_refusals_follow_the_service_order() {
    let not_child = check_add_item("WG-9", "WG-1", false, false, &[])
        .err()
        .unwrap_or_else(|| panic!("a non-child must be refused"));
    assert_eq!(not_child.code, ErrorCode::ValidationFailed);

    let member = check_add_item("WG-2", "WG-1", true, true, &[])
        .err()
        .unwrap_or_else(|| panic!("a member must be refused"));
    assert_eq!(member.code, ErrorCode::Conflict);

    let outside = vec!["beta".to_string(), "alpha".to_string()];
    let unmanaged = check_add_item("WG-3", "WG-1", true, false, &outside)
        .err()
        .unwrap_or_else(|| panic!("an unmanaged repository must be refused"));
    assert_eq!(unmanaged.code, ErrorCode::ValidationFailed);

    assert!(check_add_item("WG-4", "WG-1", true, false, &[]).is_ok());
}

#[test]
fn outside_repositories_lists_uncovered_names_sorted() {
    let (alpha, beta, gamma) = (id(1), id(2), id(3));
    let names = vec![
        (gamma, "gamma".to_string()),
        (alpha, "alpha".to_string()),
        (beta, "beta".to_string()),
    ];
    // The child links alpha and gamma; the stack covers alpha only.
    let outside = outside_repositories(&[alpha, gamma], &[alpha], &names);
    assert_eq!(outside, vec!["gamma".to_string()]);
    assert!(outside_repositories(&[beta], &[alpha], &names).contains(&"beta".to_string()));
}

/// `test_readmit_requeues_an_ejected_or_pending_child`: only ejected or pending children readmit.
#[test]
fn readmit_accepts_only_ejected_or_pending() {
    assert!(check_readmit("WG-1", S::Ejected).is_ok());
    assert!(check_readmit("WG-1", S::Pending).is_ok());
    // Readmit goes to ready, which both accepted states can reach.
    assert!(check_item_transition(S::Ejected, S::Ready).is_ok());
    assert!(check_item_transition(S::Pending, S::Ready).is_ok());

    let err = check_readmit("WG-1", S::Merged)
        .err()
        .unwrap_or_else(|| panic!("a merged child must not readmit"));
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(err.details["state"], json!("merged"));
}

/// `test_readmit_returns_an_ejected_task_to_awaiting_verification`: a ready task walks through
/// running to awaiting verification; any other status is left alone.
#[test]
fn readmit_walks_a_ready_task_back_to_verification() {
    assert_eq!(
        readmit_task_path(TaskStatus::Ready),
        &[TaskStatus::Running, TaskStatus::AwaitingVerification]
    );
    assert!(readmit_task_path(TaskStatus::Accepted).is_empty());
}

/// Ejection returns a task awaiting verification or review to ready, and parks dependents that
/// are not already ejected.
#[test]
fn ejection_returns_tasks_and_parks_dependents() {
    assert!(returns_to_ready(TaskStatus::AwaitingVerification));
    assert!(returns_to_ready(TaskStatus::AwaitingReview));
    assert!(!returns_to_ready(TaskStatus::Running));

    assert!(should_park_dependent(S::Merged));
    assert!(!should_park_dependent(S::Ejected));

    assert_eq!(
        parked_detail("WG-7"),
        json!({"merges": {}, "conflicts": {}, "reason": "dependency WG-7 was ejected"})
    );
}

/// `_eject` / `eject`: manual ejections need a reason that is not blank.
#[test]
fn manual_ejection_needs_a_reason() {
    let blank = manual_ejection("   ")
        .err()
        .unwrap_or_else(|| panic!("a blank reason must be refused"));
    assert_eq!(blank.code, ErrorCode::ValidationFailed);

    let record =
        manual_ejection("flaky timeout in the e2e suite").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        record,
        json!({"manual": true, "reason": "flaky timeout in the e2e suite"})
    );
    assert_eq!(
        manual_comment("WG-1", "human:Brice", "flaky timeout"),
        "Ejected from stack WG-1 by human:Brice: flaky timeout"
    );
}

/// `_eject`: a bisected ejection records the position, run, sorted failed checks and signatures.
#[test]
fn bisected_ejection_record_and_comment() {
    let signatures = vec![
        ("unit".to_string(), vec!["panic in parser".to_string()]),
        ("lint".to_string(), vec![]),
    ];
    let record = bisected_ejection(2, "run-42", &signatures);
    assert_eq!(record["position"], json!(2));
    assert_eq!(record["verification_run_id"], json!("run-42"));
    assert_eq!(record["failed_checks"], json!(["lint", "unit"]));
    assert_eq!(
        record["signatures"],
        json!({"lint": [], "unit": ["panic in parser"]})
    );

    let comment = bisected_comment("WG-1", "WG-5", 2, "run-42", &signatures);
    assert!(comment.contains("placed the failure at WG-5 (position 2 in the merge order)"));
    assert!(comment.contains("Failing checks: lint, unit."));
    // Signatures are listed in the order the checks were reported, as the Python does.
    assert!(comment.contains("unit: panic in parser; lint: none reported"));
}

/// The ejected item detail carries the ejection record under `ejection`.
#[test]
fn ejected_detail_wraps_the_record() {
    let record = manual_ejection("bad merge").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        ejected_detail(record),
        json!({"merges": {}, "conflicts": {}, "ejection": {"manual": true, "reason": "bad merge"}})
    );
}

/// `eject`: only a child whose merges sit on the integration branch can be ejected.
#[test]
fn only_a_merged_child_can_be_ejected() {
    assert!(check_ejectable("WG-5", "WG-1", S::Merged, true).is_ok());
    assert!(check_ejectable("WG-5", "WG-1", S::Verified, true).is_ok());

    let not_merged = check_ejectable("WG-5", "WG-1", S::Ready, true)
        .err()
        .unwrap_or_else(|| panic!("a ready child is not merged"));
    assert_eq!(not_merged.code, ErrorCode::Conflict);

    let no_merges = check_ejectable("WG-5", "WG-1", S::Merged, false)
        .err()
        .unwrap_or_else(|| panic!("a child with no recorded merges is not in the sequence"));
    assert_eq!(no_merges.code, ErrorCode::Conflict);
}

/// Stack-sibling dependencies are satisfied by ready, merged and verified items only.
#[test]
fn stack_dependency_satisfaction() {
    assert!(satisfies_stack_dependency(S::Ready));
    assert!(satisfies_stack_dependency(S::Merged));
    assert!(satisfies_stack_dependency(S::Verified));
    assert!(!satisfies_stack_dependency(S::Pending));
    assert!(!satisfies_stack_dependency(S::Ejected));
}
