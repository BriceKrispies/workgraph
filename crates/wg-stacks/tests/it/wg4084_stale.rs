//! WG-4084: a child that becomes ready after its stack's verification was queued is not on the
//! verified tip, so the landing must not release it or land the stack over it. The landing is
//! refused until the queue integrates the child and the new tip verifies.

use wg_stacks::landing::{LandDecision, LandItem, LandRepo, LandState, land_decision};
use wg_types::{StackItemState, StackStatus, TaskStatus};

const MERGED_DETAIL: &str = r#"{"merges":{"skein":{"head":"h1","merge_commit":"t1"}}}"#;

/// A verified stack: child `WG-2` is verified on the tip `t1`. `late` is the state of a child that
/// was in conflict when verification was queued and became ready while it ran.
fn stack_with_late_child(late: StackItemState) -> LandState {
    LandState {
        parent_key: "WG-1".into(),
        stack_id: "stack".into(),
        stack_status: StackStatus::Verified,
        parent_status: TaskStatus::AwaitingReview,
        items: vec![
            LandItem {
                key: "WG-2".into(),
                number: 2,
                merge_order: Some(1),
                state: StackItemState::Verified,
                task_status: TaskStatus::AwaitingReview,
                detail: Some(MERGED_DETAIL.into()),
            },
            LandItem {
                key: "WG-3".into(),
                number: 3,
                merge_order: None,
                state: late,
                task_status: TaskStatus::AwaitingVerification,
                detail: None,
            },
        ],
        verified_event: true,
        repos: vec![LandRepo {
            id: "repo".into(),
            name: "skein".into(),
            default_branch: Some("main".into()),
            base_sha: "b0".into(),
            integrated_tip: "t1".into(),
            default_head: "b0".into(),
        }],
        tips: [("skein".to_string(), "t1".to_string())]
            .into_iter()
            .collect(),
    }
}

#[test]
fn child_ready_after_verification_blocks_the_landing() {
    for late in [StackItemState::Ready, StackItemState::Pending] {
        match land_decision(&stack_with_late_child(late)) {
            Ok(decision) => panic!("{late:?} child landed over an unverified tip: {decision:?}"),
            Err(refusal) => {
                assert_eq!(refusal.code(), "stale_verification", "{late:?}");
                let error = refusal.to_error();
                assert!(error.message.contains("WG-3"), "{}", error.message);
                assert!(
                    error.message.contains("verifies the new tip"),
                    "{}",
                    error.message
                );
            }
        }
    }
}

#[test]
fn conflicting_child_is_still_released_by_the_landing() {
    // A child that was already in conflict when the verified tip was taken keeps the release
    // behaviour: the landing goes ahead and the child leaves the stack.
    match land_decision(&stack_with_late_child(StackItemState::Conflict)) {
        Ok(LandDecision::Land { children }) => assert_eq!(children, vec!["WG-2".to_string()]),
        other => panic!("expected the landing to go ahead, got {other:?}"),
    }
}
