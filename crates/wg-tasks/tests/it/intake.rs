//! The intake stage: the `proposed` status, its transitions, how a create lands in it, its
//! exclusion from the ready and promote-ready selections, and the intake rules.

use crate::common::{self, id, stamp, task, tasks_slice};
use common::tag;
use serde_json::json;
use wg_model::slice::StackSlice;
use wg_model::stamp::Stamp;
use wg_tasks::create::{Allocation, NewTask, create_plan};
use wg_tasks::intake::{
    admit_status, dropped_reason, intake_enabled, merge_comment, merged_reason, require_proposed,
};
use wg_tasks::lifecycle::check_task_transition;
use wg_tasks::promote::{
    PromoteScope, PromoteSkip, ReadyQuery, new_task_skip, promote_ready, promote_report,
    ready_tasks,
};
use wg_types::{Actor, ErrorCode, TaskPriority, TaskStatus};

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

fn new_task(status: TaskStatus) -> NewTask {
    NewTask {
        key: None,
        title: "Follow-up".to_owned(),
        description: "Found in review".to_owned(),
        acceptance_criteria: vec!["it works".to_owned()],
        status,
        priority: TaskPriority::Medium,
        parent_id: None,
        assignee: None,
        assigned_agent: None,
        auto_execute: false,
        due_at: None,
        scheduled_at: None,
        estimated_effort: None,
        metadata: serde_json::Map::new(),
        tags: vec![],
        files: vec![],
        projects: vec![],
        repositories: vec![],
        depends_on: vec![],
    }
}

fn plan(status: TaskStatus) -> wg_tasks::create::CreatePlan {
    let allocation = Allocation {
        workspace_id: common::workspace(),
        key_prefix: "WG",
        number: 1,
        key_taken: false,
    };
    create_plan(&stamp_for("agent:claude"), allocation, new_task(status))
        .unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn a_proposed_create_is_inserted_proposed_and_is_not_moved_to_ready() {
    let proposed = plan(TaskStatus::Proposed);
    assert_eq!(proposed.task.status, TaskStatus::Proposed);
    assert_eq!(proposed.event.status, TaskStatus::Proposed);
    assert!(!proposed.ready);

    let ordinary = plan(TaskStatus::Backlog);
    assert_eq!(ordinary.task.status, TaskStatus::Backlog);
    assert_eq!(ordinary.event.status, TaskStatus::Backlog);
    assert!(plan(TaskStatus::Ready).ready);
}

#[test]
fn proposed_moves_only_to_backlog_ready_or_cancelled() {
    for target in [
        TaskStatus::Backlog,
        TaskStatus::Ready,
        TaskStatus::Cancelled,
    ] {
        assert!(check_task_transition(TaskStatus::Proposed, target, false, false).is_ok());
    }
    for target in [
        TaskStatus::Blocked,
        TaskStatus::Claimed,
        TaskStatus::Running,
        TaskStatus::AwaitingVerification,
        TaskStatus::AwaitingReview,
        TaskStatus::Accepted,
        TaskStatus::Failed,
    ] {
        let err = check_task_transition(TaskStatus::Proposed, target, false, false).err();
        assert_eq!(err.map(|e| e.code), Some(ErrorCode::InvalidTransition));
    }
}

#[test]
fn nothing_moves_into_proposed() {
    for from in [
        TaskStatus::Backlog,
        TaskStatus::Ready,
        TaskStatus::Blocked,
        TaskStatus::Claimed,
        TaskStatus::Running,
        TaskStatus::Failed,
        TaskStatus::Accepted,
        TaskStatus::Cancelled,
    ] {
        let err = check_task_transition(from, TaskStatus::Proposed, false, false).err();
        assert_eq!(
            err.map(|e| e.code),
            Some(ErrorCode::InvalidTransition),
            "{from}"
        );
    }
}

#[test]
fn proposed_tasks_are_not_ready_or_promotable() {
    let slice = tasks_slice(
        vec![
            task("WG", 1, TaskStatus::Proposed, None),
            task("WG", 2, TaskStatus::Backlog, None),
            task("WG", 3, TaskStatus::Ready, None),
        ],
        vec![],
        vec![],
    );
    let stacks = StackSlice::default();
    let promoted = promote_ready(
        &slice,
        &stacks,
        common::workspace(),
        PromoteScope {
            tags: &[],
            project: None,
        },
    );
    let keys: Vec<String> = promoted.iter().map(|t| t.key.to_string()).collect();
    assert_eq!(keys, ["WG-2"], "only the backlog task is promoted");

    let ready = ready_tasks(
        &slice,
        &stacks,
        ReadyQuery {
            workspace_id: common::workspace(),
            project: None,
            repository: None,
            agent: None,
            limit: 10,
        },
    );
    let keys: Vec<String> = ready.iter().map(|t| t.key.to_string()).collect();
    assert_eq!(keys, ["WG-3"], "only the ready task is offered");
}

#[test]
fn intake_is_off_unless_the_workspace_turns_it_on() {
    assert!(!intake_enabled(&serde_json::Map::new()));
    let on = json!({"tasks": {"intake_proposed": true}});
    assert!(intake_enabled(
        on.as_object().unwrap_or(&serde_json::Map::new())
    ));
    let off = json!({"tasks": {"intake_proposed": false}});
    assert!(!intake_enabled(
        off.as_object().unwrap_or(&serde_json::Map::new())
    ));
}

#[test]
fn intake_acts_on_proposed_tasks_only() {
    assert!(require_proposed(&task("WG", 1, TaskStatus::Proposed, None)).is_ok());
    let err = require_proposed(&task("WG", 2, TaskStatus::Backlog, None)).err();
    let err = err.unwrap_or_else(|| panic!("a backlog task is not proposed"));
    assert_eq!(err.code, ErrorCode::InvalidTransition);
    assert!(err.message.contains("WG-2"));
}

#[test]
fn admit_goes_to_backlog_or_ready_with_the_flag() {
    assert_eq!(admit_status(false), TaskStatus::Backlog);
    assert_eq!(admit_status(true), TaskStatus::Ready);
}

#[test]
fn a_merge_comment_carries_the_description_and_criteria() {
    let mut proposal = task("WG", 7, TaskStatus::Proposed, None);
    proposal.title = "Handle empty input".to_owned();
    proposal.description = "The parser panics on an empty file.".to_owned();
    proposal.acceptance_criteria = rpds::VectorSync::from_iter(["no panic".to_owned()]);
    let body = merge_comment(&proposal);
    assert!(body.contains("WG-7"));
    assert!(body.contains("Handle empty input"));
    assert!(body.contains("The parser panics on an empty file."));
    assert!(body.contains("- no panic"));
    assert_eq!(merged_reason("WG-3"), "merged into WG-3 at intake");
    assert_eq!(
        dropped_reason("out of scope"),
        "dropped at intake: out of scope"
    );
}

#[test]
fn promote_report_names_why_each_candidate_stays_where_it_is() {
    // WG-1 proposed; WG-2 has child WG-3; WG-4 parked; WG-5 waits for WG-6 (ready, not accepted).
    let slice = tasks_slice(
        vec![
            task("WG", 1, TaskStatus::Proposed, None),
            task("WG", 2, TaskStatus::Backlog, None),
            task("WG", 3, TaskStatus::Backlog, Some(id(2))),
            task("WG", 4, TaskStatus::Backlog, None),
            task("WG", 5, TaskStatus::Backlog, None),
            task("WG", 6, TaskStatus::Ready, None),
        ],
        vec![tag(id(4), "parked")],
        vec![(id(5), id(6))],
    );
    let stacks = StackSlice::default();
    let report = promote_report(
        &slice,
        &stacks,
        common::workspace(),
        PromoteScope {
            tags: &[],
            project: None,
        },
    );
    let reasons: Vec<(String, Option<PromoteSkip>)> = report
        .into_iter()
        .map(|(task, skip)| (task.key.to_string(), skip))
        .collect();
    assert_eq!(
        reasons,
        vec![
            ("WG-1".to_owned(), Some(PromoteSkip::Proposed)),
            ("WG-2".to_owned(), Some(PromoteSkip::HasChildren)),
            ("WG-3".to_owned(), None),
            ("WG-4".to_owned(), Some(PromoteSkip::Parked)),
            (
                "WG-5".to_owned(),
                Some(PromoteSkip::Dependencies(vec!["WG-6".to_owned()]))
            ),
        ],
        "WG-6 is ready, not in promote scope"
    );
    let text = PromoteSkip::Dependencies(vec!["WG-6".to_owned()]).reason();
    assert!(text.contains("WG-6"), "{text}");
}

#[test]
fn a_new_task_is_skipped_for_parked_tags_and_unaccepted_dependencies_only() {
    let slice = tasks_slice(
        vec![
            task("WG", 6, TaskStatus::Ready, None),
            task("WG", 7, TaskStatus::Accepted, None),
        ],
        vec![],
        vec![],
    );
    assert_eq!(
        new_task_skip(&slice, &["parked".to_owned()], &[]),
        Some(PromoteSkip::Parked)
    );
    assert_eq!(
        new_task_skip(&slice, &[], &[id(6)]),
        Some(PromoteSkip::Dependencies(vec!["WG-6".to_owned()]))
    );
    assert_eq!(new_task_skip(&slice, &[], &[id(7)]), None);
    assert_eq!(new_task_skip(&slice, &[], &[]), None);
}
