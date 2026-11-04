//! `task close-delivered` (`deliver_task`): a backlog, blocked or ready task recorded as already
//! delivered. The close needs an evidence reason, drops only the dependency edges the caller
//! names, records each dropped edge, and records the acceptance and the status change with the
//! actor. A cancelled dependency never blocks it.

use crate::common;

use common::{id, stack_slice, stamp, task, tasks_slice};
use wg_model::event::EventPayload;
use wg_model::stamp::Stamp;
use wg_model::{Row, TaskSlice, Write};
use wg_tasks::accept::{DELIVERED_MODE, DeliveredClose, deliver_task, is_commit_sha};
use wg_types::{Actor, ErrorCode, TaskStatus};

fn stamp_for(text: &str) -> Stamp {
    Stamp {
        at: stamp(100),
        entropy: [7; 32],
        actor: Actor::parse(text).unwrap_or_else(|e| panic!("{e}")),
        permissions: rpds::RedBlackTreeSetSync::new_sync(),
        correlation: id(9).into(),
        causation: None,
        attempt: None,
        automation_depth: 0,
    }
}

/// Task 2 depends on task 1, with task 1 in `blocker` status.
fn pair(dependent: TaskStatus, blocker: TaskStatus) -> TaskSlice {
    tasks_slice(
        vec![
            task("TCD", 1, blocker, None),
            task("TCD", 2, dependent, None),
        ],
        vec![],
        vec![(id(2), id(1))],
    )
}

fn close(reason: &str, drop: Vec<u128>) -> DeliveredClose {
    DeliveredClose {
        reason: reason.to_owned(),
        commit: None,
        drop: drop.into_iter().map(id).collect(),
    }
}

/// The slice after the decision's writes.
fn after(tasks: &TaskSlice, writes: &[Write]) -> TaskSlice {
    tasks.apply_all(writes.iter())
}

#[test]
fn closes_backlog_blocked_and_ready_tasks_as_accepted() {
    let stacks = stack_slice(vec![]);
    for status in [TaskStatus::Backlog, TaskStatus::Blocked, TaskStatus::Ready] {
        let tasks = tasks_slice(vec![task("TCD", 1, status, None)], vec![], vec![]);
        let stamp = stamp_for("human:brice");
        let (decision, acceptance) = deliver_task(
            &tasks,
            &stacks,
            &stamp,
            id(1),
            &close("merged in the release branch", vec![]),
        )
        .unwrap_or_else(|e| panic!("{status}: {e}"));

        let done = after(&tasks, &decision.writes.iter().cloned().collect::<Vec<_>>());
        let closed = done.task(id(1)).unwrap_or_else(|| panic!("task 1"));
        assert_eq!(closed.status, TaskStatus::Accepted, "from {status}");
        assert!(closed.accepted_at.is_some(), "from {status}");

        assert_eq!(acceptance.mode, DELIVERED_MODE);
        assert_eq!(acceptance.decision, "accepted");
        assert_eq!(acceptance.reason, "merged in the release branch");
        assert_eq!(acceptance.actor, "human:brice");
        assert!(acceptance.attempt_id.is_none());
        assert!(acceptance.verification_run_id.is_none());

        let writes: Vec<&Write> = decision.writes.iter().collect();
        assert!(
            writes.iter().any(|write| matches!(
                write,
                Write::Insert(Row::TaskAcceptance(row)) if row.mode == DELIVERED_MODE
            )),
            "the acceptance row is written from {status}"
        );
        let payloads: Vec<&EventPayload> = decision.events.iter().map(|e| &e.payload).collect();
        assert!(
            payloads.iter().any(|p| matches!(
                p,
                EventPayload::TaskAccepted(accepted) if accepted.reason == "merged in the release branch"
            )),
            "task.accepted is recorded from {status}"
        );
        let status_reason = decision
            .events
            .iter()
            .find_map(|event| match &event.payload {
                EventPayload::TaskStatusChanged(changed) => Some(changed.reason.clone()),
                _ => None,
            });
        assert_eq!(
            status_reason.flatten().as_deref(),
            Some("already delivered: merged in the release branch"),
            "the status change carries the evidence from {status}"
        );
    }
}

#[test]
fn refuses_a_missing_or_blank_reason() {
    let stacks = stack_slice(vec![]);
    let tasks = tasks_slice(
        vec![task("TCD", 1, TaskStatus::Ready, None)],
        vec![],
        vec![],
    );
    for reason in ["", "   "] {
        let err = deliver_task(
            &tasks,
            &stacks,
            &stamp_for("human:brice"),
            id(1),
            &close(reason, vec![]),
        )
        .err()
        .unwrap_or_else(|| panic!("a blank reason must be refused"));
        assert_eq!(err.code, ErrorCode::ValidationFailed);
        assert!(err.message.contains("evidence reason"), "{}", err.message);
    }
}

#[test]
fn refuses_statuses_that_have_their_own_path() {
    let stacks = stack_slice(vec![]);
    for status in [
        TaskStatus::AwaitingReview,
        TaskStatus::AwaitingVerification,
        TaskStatus::Claimed,
        TaskStatus::Running,
        TaskStatus::Failed,
        TaskStatus::Cancelled,
        TaskStatus::Accepted,
    ] {
        let tasks = tasks_slice(vec![task("TCD", 1, status, None)], vec![], vec![]);
        let err = deliver_task(
            &tasks,
            &stacks,
            &stamp_for("human:brice"),
            id(1),
            &close("done", vec![]),
        )
        .err()
        .unwrap_or_else(|| panic!("{status} must be refused"));
        assert_eq!(err.code, ErrorCode::InvalidTransition, "from {status}");
    }
}

#[test]
fn refuses_unaccepted_dependencies_unless_they_are_named_to_drop() {
    let stacks = stack_slice(vec![]);
    let tasks = pair(TaskStatus::Backlog, TaskStatus::Ready);
    let err = deliver_task(
        &tasks,
        &stacks,
        &stamp_for("human:brice"),
        id(2),
        &close("done elsewhere", vec![]),
    )
    .err()
    .unwrap_or_else(|| panic!("an unaccepted dependency must refuse the close"));
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(
        err.details.get("blocked_by"),
        Some(&serde_json::json!(["TCD-1"]))
    );
}

#[test]
fn a_cancelled_dependency_does_not_block_the_close() {
    let stacks = stack_slice(vec![]);
    let tasks = pair(TaskStatus::Backlog, TaskStatus::Cancelled);
    let (decision, _) = deliver_task(
        &tasks,
        &stacks,
        &stamp_for("human:brice"),
        id(2),
        &close("done elsewhere", vec![]),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let done = after(&tasks, &decision.writes.iter().cloned().collect::<Vec<_>>());
    assert_eq!(
        done.task(id(2)).map(|t| t.status),
        Some(TaskStatus::Accepted)
    );
    // The cancelled edge is not dropped: it never blocked, so the close leaves it alone.
    assert_eq!(done.dependencies_of(id(2)).count(), 1);
}

#[test]
fn dropping_a_named_edge_records_its_removal_and_then_closes() {
    let stacks = stack_slice(vec![]);
    let tasks = pair(TaskStatus::Backlog, TaskStatus::Ready);
    let (decision, acceptance) = deliver_task(
        &tasks,
        &stacks,
        &stamp_for("human:brice"),
        id(2),
        &close("superseded by TCD-1 in the same commit", vec![1]),
    )
    .unwrap_or_else(|e| panic!("{e}"));

    let removed: Vec<_> = decision
        .events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::TaskDependencyRemoved(changed) => {
                Some(changed.depends_on_key.to_string())
            }
            _ => None,
        })
        .collect();
    assert_eq!(removed, ["TCD-1"], "each dropped edge is recorded");

    let done = after(&tasks, &decision.writes.iter().cloned().collect::<Vec<_>>());
    assert_eq!(done.dependencies_of(id(2)).count(), 0, "the edge is gone");
    assert_eq!(
        done.task(id(2)).map(|t| t.status),
        Some(TaskStatus::Accepted)
    );
    assert_eq!(acceptance.task_id, id(2));
}

#[test]
fn dropping_a_name_that_is_not_a_dependency_is_refused() {
    let stacks = stack_slice(vec![]);
    let tasks = pair(TaskStatus::Backlog, TaskStatus::Ready);
    let err = deliver_task(
        &tasks,
        &stacks,
        &stamp_for("human:brice"),
        id(1),
        &close("done", vec![2]),
    )
    .err()
    .unwrap_or_else(|| panic!("task 2 is not a dependency of task 1"));
    assert_eq!(err.code, ErrorCode::ValidationFailed);
}

#[test]
fn records_the_commit_on_the_status_change_and_rejects_a_bad_sha() {
    let stacks = stack_slice(vec![]);
    let tasks = tasks_slice(
        vec![task("TCD", 1, TaskStatus::Ready, None)],
        vec![],
        vec![],
    );
    let with_commit = DeliveredClose {
        reason: "shipped".to_owned(),
        commit: Some("9f2c7a2".to_owned()),
        drop: vec![],
    };
    let (decision, _) = deliver_task(
        &tasks,
        &stacks,
        &stamp_for("human:brice"),
        id(1),
        &with_commit,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let commit = decision
        .events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::TaskStatusChanged(changed) => changed.extra.get("commit").cloned(),
            _ => None,
        });
    assert_eq!(commit, Some(serde_json::json!("9f2c7a2")));

    assert!(is_commit_sha("9f2c7a2"));
    assert!(!is_commit_sha("zz12"));
    assert!(!is_commit_sha("abc"));
    let bad = DeliveredClose {
        reason: "shipped".to_owned(),
        commit: Some("not-a-sha".to_owned()),
        drop: vec![],
    };
    let err = deliver_task(&tasks, &stacks, &stamp_for("human:brice"), id(1), &bad)
        .err()
        .unwrap_or_else(|| panic!("a non-hex commit must be refused"));
    assert_eq!(err.code, ErrorCode::ValidationFailed);
}
