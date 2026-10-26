//! CORE-02: the propagation decisions that the GV-19 vectors cannot observe, because the Python
//! service applies them as side effects: the dependency gate of a status change, the refresh of a
//! dependent when a dependency is accepted or reopened, and promotion of the dependents.

mod common;

use common::{id, stack_slice, task, tasks_slice};
use serde_json::json;
use wg_tasks::promote::{
    REASON_REOPENED, REASON_UNBLOCKED, Refresh, dependency_gate, promote_dependents,
    refresh_blocked_state,
};
use wg_types::{ErrorCode, StackItemState, StackStatus, TaskStatus};

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
