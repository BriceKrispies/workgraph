//! CORE-02: the propagation decisions that the GV-19 vectors cannot observe, because the Python
//! service applies them as side effects: the dependency gate of a status change, the refresh of a
//! dependent when a dependency is accepted or reopened, and promotion of the dependents.

use crate::common;

use common::{id, stack_slice, stamp, task, tasks_slice};
use serde_json::json;
use wg_model::stamp::Stamp;
use wg_tasks::deps::{
    CLAIMABLE_DEPENDENCY_STATUSES, unaccepted_dependencies, unsatisfied_dependencies,
    unsatisfied_dependencies_in,
};
use wg_tasks::promote::{
    REASON_REOPENED, REASON_UNBLOCKED, Refresh, dependency_gate, dependency_gate_in,
    promote_dependents, refresh_blocked_state,
};
use wg_tasks::transition::{Change, transition};
use wg_types::{Actor, ErrorCode, StackItemState, StackStatus, TaskStatus};

/// Task 2 depends on task 1; task 1 has `first` status and task 2 `second`.
fn pair(first: TaskStatus, second: TaskStatus) -> wg_model::TaskSlice {
    tasks_slice(
        vec![task("TAA", 1, first, None), task("TAA", 2, second, None)],
        vec![],
        vec![(id(2), id(1))],
    )
}

#[test]
fn gate_turns_ready_into_blocked_with_the_blockers_as_reason() {
    let tasks = pair(TaskStatus::Ready, TaskStatus::Backlog);
    let stacks = stack_slice(vec![]);
    let dependent = tasks.task(id(2)).unwrap_or_else(|| panic!("task 2"));
    let gated = dependency_gate(&tasks, &stacks, dependent, TaskStatus::Ready, None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(gated.target, TaskStatus::Blocked);
    assert!(gated.blocked_by_dependencies);
    assert_eq!(
        gated.reason.as_deref(),
        Some("dependencies unsatisfied: TAA-1")
    );

    let kept = dependency_gate(&tasks, &stacks, dependent, TaskStatus::Ready, Some("why"))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(kept.reason.as_deref(), Some("why"));
}

#[test]
fn gate_refuses_claim_and_run_while_blocked() {
    let tasks = pair(TaskStatus::Ready, TaskStatus::Ready);
    let stacks = stack_slice(vec![]);
    let dependent = tasks.task(id(2)).unwrap_or_else(|| panic!("task 2"));
    for target in [TaskStatus::Claimed, TaskStatus::Running] {
        let err = dependency_gate(&tasks, &stacks, dependent, target, None)
            .err()
            .unwrap_or_else(|| panic!("{target} must be refused"));
        assert_eq!(err.code, ErrorCode::InvalidTransition);
        assert_eq!(err.message, "task TAA-2 has unsatisfied dependencies");
        assert_eq!(err.details.get("blocked_by"), Some(&json!(["TAA-1"])));
    }
}

#[test]
fn gate_passes_other_targets_and_satisfied_dependencies() {
    let stacks = stack_slice(vec![]);
    let open = pair(TaskStatus::Ready, TaskStatus::Backlog);
    let dependent = open.task(id(2)).unwrap_or_else(|| panic!("task 2"));
    let cancelled = dependency_gate(&open, &stacks, dependent, TaskStatus::Cancelled, None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(cancelled.target, TaskStatus::Cancelled);
    assert!(!cancelled.blocked_by_dependencies);

    let done = pair(TaskStatus::Accepted, TaskStatus::Backlog);
    let dependent = done.task(id(2)).unwrap_or_else(|| panic!("task 2"));
    let ready = dependency_gate(&done, &stacks, dependent, TaskStatus::Ready, None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(ready.target, TaskStatus::Ready);
}

#[test]
fn a_stack_sibling_item_satisfies_the_gate() {
    let tasks = pair(TaskStatus::Running, TaskStatus::Backlog);
    let stacks = stack_slice(vec![(
        id(9),
        StackStatus::Open,
        vec![
            (id(1), StackItemState::Ready),
            (id(2), StackItemState::Pending),
        ],
    )]);
    let dependent = tasks.task(id(2)).unwrap_or_else(|| panic!("task 2"));
    let gated = dependency_gate(&tasks, &stacks, dependent, TaskStatus::Running, None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(gated.target, TaskStatus::Running);
}

#[test]
fn refresh_unblocks_only_tasks_parked_by_dependencies() {
    let stacks = stack_slice(vec![]);
    let done = pair(TaskStatus::Accepted, TaskStatus::Blocked);

    // Blocked without the dependencies marker (blocked by hand): left alone.
    let manual = done.task(id(2)).unwrap_or_else(|| panic!("task 2"));
    assert_eq!(refresh_blocked_state(&done, &stacks, manual), None);

    // Blocked by dependencies: becomes ready now that task 1 is accepted.
    let marked = wg_model::Task {
        metadata: serde_json::Map::from_iter([(
            "blocked_reason".to_owned(),
            json!("dependencies"),
        )]),
        ..manual.clone()
    };
    assert_eq!(
        refresh_blocked_state(&done, &stacks, &marked),
        Some(Refresh {
            target: TaskStatus::Ready,
            reason: REASON_UNBLOCKED,
        })
    );
}

#[test]
fn refresh_blocks_a_ready_task_again_when_a_dependency_reopens() {
    let stacks = stack_slice(vec![]);
    let reopened = pair(TaskStatus::Ready, TaskStatus::Ready);
    let dependent = reopened.task(id(2)).unwrap_or_else(|| panic!("task 2"));
    assert_eq!(
        refresh_blocked_state(&reopened, &stacks, dependent),
        Some(Refresh {
            target: TaskStatus::Ready,
            reason: REASON_REOPENED,
        })
    );
    let accepted = pair(TaskStatus::Accepted, TaskStatus::Ready);
    let dependent = accepted.task(id(2)).unwrap_or_else(|| panic!("task 2"));
    assert_eq!(refresh_blocked_state(&accepted, &stacks, dependent), None);
}

#[test]
fn promote_dependents_moves_backlog_dependents_whose_dependencies_are_accepted() {
    let stacks = stack_slice(vec![]);
    // Task 1 is accepted. Task 2 depends on it alone; task 3 also on task 4, still open; task 5
    // is parked.
    let tasks = tasks_slice(
        vec![
            task("TAA", 1, TaskStatus::Accepted, None),
            task("TAA", 2, TaskStatus::Backlog, None),
            task("TAA", 3, TaskStatus::Backlog, None),
            task("TAA", 4, TaskStatus::Ready, None),
            task("TAA", 5, TaskStatus::Backlog, None),
        ],
        vec![common::tag(id(5), "parked")],
        vec![
            (id(2), id(1)),
            (id(3), id(1)),
            (id(3), id(4)),
            (id(5), id(1)),
        ],
    );
    let promoted = promote_dependents(&tasks, &stacks, id(1));
    let keys: Vec<String> = promoted.iter().map(|t| t.key.to_string()).collect();
    assert_eq!(keys, ["TAA-2"]);
}

fn propagation_stamp() -> Stamp {
    Stamp {
        at: stamp(100),
        entropy: [7; 32],
        actor: Actor::parse("human:brice").unwrap_or_else(|e| panic!("{e}")),
        permissions: rpds::RedBlackTreeSetSync::new_sync(),
        correlation: id(9).into(),
        causation: None,
        attempt: None,
        automation_depth: 0,
    }
}

#[test]
fn a_cancelled_dependency_is_satisfied() {
    let stacks = stack_slice(vec![]);
    let tasks = pair(TaskStatus::Cancelled, TaskStatus::Backlog);
    assert!(unsatisfied_dependencies(&tasks, &stacks, id(2)).is_empty());

    let dependent = tasks.task(id(2)).unwrap_or_else(|| panic!("task 2"));
    let gated = dependency_gate(&tasks, &stacks, dependent, TaskStatus::Ready, None)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(gated.target, TaskStatus::Ready);
    assert!(!gated.blocked_by_dependencies);

    let promoted = promote_dependents(&tasks, &stacks, id(1));
    let keys: Vec<String> = promoted.iter().map(|t| t.key.to_string()).collect();
    assert_eq!(keys, ["TAA-2"]);
}

#[test]
fn cancelling_a_dependency_unblocks_its_dependent() {
    let stacks = stack_slice(vec![]);
    let mut parked = task("TAA", 2, TaskStatus::Blocked, None);
    parked.metadata =
        serde_json::Map::from_iter([("blocked_reason".to_owned(), json!("dependencies"))]);
    let tasks = tasks_slice(
        vec![task("TAA", 1, TaskStatus::Ready, None), parked],
        vec![],
        vec![(id(2), id(1))],
    );
    let applied = transition(
        &tasks,
        &stacks,
        &propagation_stamp(),
        propagation_stamp().ids(),
        id(1),
        TaskStatus::Cancelled,
        Change::default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let dependent = applied
        .tasks
        .task(id(2))
        .unwrap_or_else(|| panic!("task 2"));
    assert_eq!(dependent.status, TaskStatus::Ready);
}

fn blocked_by_dependencies(number: i64) -> wg_model::Task {
    let mut parked = task("TAA", number, TaskStatus::Blocked, None);
    parked.metadata =
        serde_json::Map::from_iter([("blocked_reason".to_owned(), json!("dependencies"))]);
    parked
}

#[test]
fn a_completed_blocker_satisfies_a_manual_claim_but_not_scheduling() {
    let stacks = stack_slice(vec![]);
    for complete in [TaskStatus::AwaitingReview, TaskStatus::AwaitingVerification] {
        let tasks = pair(complete, TaskStatus::Ready);
        let dependent = tasks.task(id(2)).unwrap_or_else(|| panic!("task 2"));
        // Scheduling waits for acceptance: the task is still blocked and cannot be claimed.
        assert_eq!(unsatisfied_dependencies(&tasks, &stacks, id(2)).len(), 1);
        let err = dependency_gate(&tasks, &stacks, dependent, TaskStatus::Claimed, None)
            .err()
            .unwrap_or_else(|| panic!("scheduling must refuse {complete}"));
        assert_eq!(err.code, ErrorCode::InvalidTransition);
        // A manual claim accepts the completed blocker.
        assert!(
            unsatisfied_dependencies_in(&tasks, &stacks, id(2), CLAIMABLE_DEPENDENCY_STATUSES)
                .is_empty()
        );
        let claimed = dependency_gate_in(
            &tasks,
            &stacks,
            dependent,
            TaskStatus::Claimed,
            None,
            CLAIMABLE_DEPENDENCY_STATUSES,
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(claimed.target, TaskStatus::Claimed);
    }
    // A claimed blocker satisfies neither.
    let tasks = pair(TaskStatus::Claimed, TaskStatus::Ready);
    assert_eq!(
        unsatisfied_dependencies_in(&tasks, &stacks, id(2), CLAIMABLE_DEPENDENCY_STATUSES).len(),
        1
    );
}

#[test]
fn a_blocker_awaiting_review_leaves_the_dependent_blocked_until_accepted() {
    let stacks = stack_slice(vec![]);
    let tasks = tasks_slice(
        vec![
            task("TAA", 1, TaskStatus::Claimed, None),
            blocked_by_dependencies(2),
        ],
        vec![],
        vec![(id(2), id(1))],
    );
    let completed = transition(
        &tasks,
        &stacks,
        &propagation_stamp(),
        propagation_stamp().ids(),
        id(1),
        TaskStatus::AwaitingReview,
        Change::default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let status_of =
        |applied: &wg_tasks::transition::Applied| applied.tasks.task(id(2)).map(|t| t.status);
    assert_eq!(status_of(&completed), Some(TaskStatus::Blocked));

    let accepted = transition(
        &completed.tasks,
        &stacks,
        &propagation_stamp(),
        completed.ids,
        id(1),
        TaskStatus::Accepted,
        Change {
            privileged: true,
            ..Change::default()
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(status_of(&accepted), Some(TaskStatus::Ready));
}

#[test]
fn a_claim_transition_takes_a_task_whose_blocker_awaits_review() {
    let stacks = stack_slice(vec![]);
    let tasks = pair(TaskStatus::AwaitingReview, TaskStatus::Ready);
    let refused = transition(
        &tasks,
        &stacks,
        &propagation_stamp(),
        propagation_stamp().ids(),
        id(2),
        TaskStatus::Claimed,
        Change::default(),
    );
    assert!(refused.is_err());
    let claimed = transition(
        &tasks,
        &stacks,
        &propagation_stamp(),
        propagation_stamp().ids(),
        id(2),
        TaskStatus::Claimed,
        Change {
            claimable: true,
            ..Change::default()
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        claimed.tasks.task(id(2)).map(|t| t.status),
        Some(TaskStatus::Claimed)
    );
}

#[test]
fn acceptance_needs_every_direct_dependency_accepted() {
    let tasks = tasks_slice(
        vec![
            task("TAA", 1, TaskStatus::Accepted, None),
            task("TAA", 2, TaskStatus::AwaitingReview, None),
            task("TAA", 3, TaskStatus::AwaitingVerification, None),
            task("TAA", 4, TaskStatus::Cancelled, None),
            task("TAA", 5, TaskStatus::AwaitingReview, None),
        ],
        vec![],
        vec![
            (id(5), id(1)),
            (id(5), id(2)),
            (id(5), id(3)),
            (id(5), id(4)),
        ],
    );
    let keys: Vec<String> = unaccepted_dependencies(&tasks, id(5))
        .iter()
        .map(|t| t.key.to_string())
        .collect();
    assert_eq!(keys, ["TAA-2", "TAA-3", "TAA-4"]);
    assert!(unaccepted_dependencies(&tasks, id(1)).is_empty());
}
