//! CORE-03: task writes ported from `tests/unit/test_tasks_core.py` (optimistic concurrency,
//! bulk update, tags and files) and the tracked-task rules of `tests/integration/test_tracker_ownership.py`.

mod common;

use common::{id, stamp, task, tasks_slice};
use rpds::VectorSync;
use wg_model::entity::ExternalItem;
use wg_model::stamp::Stamp;
use wg_model::{Row, TaskSlice, Write};
use wg_tasks::archive::{ArchiveChange, archive_plan};
use wg_tasks::bulk::bulk_results;
use wg_tasks::create::{Allocation, NewTask, create_plan};
use wg_tasks::update::{TaskPatch, bulk_patch, update_plan};
use wg_types::{Actor, ErrorCode, TaskPriority, TaskStatus};

fn actor(text: &str) -> Actor {
    Actor::parse(text).unwrap_or_else(|e| panic!("{e}"))
}

fn stamp_for(text: &str) -> Stamp {
    Stamp {
        at: stamp(100),
        entropy: [7; 32],
        actor: actor(text),
        permissions: rpds::RedBlackTreeSetSync::new_sync(),
        correlation: id(9).into(),
        causation: None,
        attempt: None,
        automation_depth: 0,
    }
}

/// A tracked task (`ABC-1` in the workspace `WG`) mirrored from the Jira issue `ABC-1`.
fn tracked() -> (TaskSlice, wg_model::entity::Task) {
    let mut row = task("WG", 1, TaskStatus::Ready, None);
    row.title = "Mirrored work".to_owned();
    let item = ExternalItem {
        id: id(0x5000),
        workspace_id: common::workspace(),
        task_id: row.id,
        connection: "work-jira".to_owned(),
        external_id: "id-1".to_owned(),
        external_key: "ABC-1".to_owned(),
        url: String::new(),
        tracker_status: "To Do".to_owned(),
        tracker_status_category: "new".to_owned(),
        tracker_resolution: None,
        remote_updated_at: None,
        snapshot: serde_json::Map::new(),
        snapshot_hash: String::new(),
        synced_at: None,
        missing_since: None,
        created_at: stamp(0),
        updated_at: stamp(0),
    };
    let slice = tasks_slice(vec![row.clone()], vec![], vec![]);
    let slice = slice.apply_all(&[Write::Insert(Row::ExternalItem(item))]);
    (slice, row)
}

fn empty_patch() -> TaskPatch {
    TaskPatch::default()
}

#[test]
fn stale_expected_version_is_a_version_conflict() {
    let slice = tasks_slice(
        vec![task("WG", 1, TaskStatus::Backlog, None)],
        vec![],
        vec![],
    );
    let row = slice.task(id(1)).unwrap_or_else(|| panic!("task 1"));
    let stamp = stamp_for("human:brice");
    let patch = TaskPatch {
        expected_version: Some(row.version + 5),
        title: Some("stale".to_owned()),
        ..empty_patch()
    };
    let err = update_plan(&stamp, &slice, row, &patch).err();
    assert_eq!(err.map(|e| e.code), Some(ErrorCode::VersionConflict));

    let ok = TaskPatch {
        expected_version: Some(row.version),
        title: Some("T2".to_owned()),
        ..empty_patch()
    };
    let plan = update_plan(&stamp, &slice, row, &ok).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(plan.task.version, row.version + 1);
    assert!(plan.event.is_some());
}

#[test]
fn update_records_only_real_changes_and_flags_material_edits() {
    let slice = tasks_slice(
        vec![task("WG", 1, TaskStatus::Backlog, None)],
        vec![],
        vec![],
    );
    let row = slice.task(id(1)).unwrap_or_else(|| panic!("task 1"));
    let stamp = stamp_for("human:brice");

    let same = TaskPatch {
        title: Some(row.title.clone()),
        ..empty_patch()
    };
    let plan = update_plan(&stamp, &slice, row, &same).unwrap_or_else(|e| panic!("{e}"));
    assert!(plan.event.is_none());
    assert_eq!(plan.task.version, row.version);

    let reflowed = TaskPatch {
        description: Some("  Some   text ".to_owned()),
        ..empty_patch()
    };
    let plan = update_plan(&stamp, &slice, row, &reflowed).unwrap_or_else(|e| panic!("{e}"));
    assert!(plan.material_change);
    assert_eq!(plan.changes.0.len(), 1);
}

#[test]
fn tags_add_and_remove_keep_the_stored_order() {
    let slice = common::tasks_slice(
        vec![task("WG", 1, TaskStatus::Backlog, None)],
        vec![common::tag(id(1), "a"), common::tag(id(1), "b")],
        vec![],
    );
    let row = slice.task(id(1)).unwrap_or_else(|| panic!("task 1"));
    let patch = TaskPatch {
        add_tags: vec!["c".to_owned(), "a".to_owned()],
        remove_tags: vec!["b".to_owned()],
        ..empty_patch()
    };
    let plan = update_plan(&stamp_for("human:brice"), &slice, row, &patch)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(plan.tags, Some(vec!["a".to_owned(), "c".to_owned()]));
}

#[test]
fn bulk_patch_drops_the_version_and_failures_are_listed() {
    let guarded = TaskPatch {
        expected_version: Some(3),
        add_tags: vec!["bulk".to_owned()],
        ..empty_patch()
    };
    assert_eq!(bulk_patch(&guarded).expected_version, None);
    assert_eq!(bulk_patch(&guarded).add_tags, vec!["bulk".to_owned()]);

    let result = bulk_results(vec![
        ("WG-1".to_owned(), Ok("WG-1".to_owned())),
        (
            "WG-404".to_owned(),
            Err(wg_types::WgError::new(ErrorCode::NotFound, "missing")),
        ),
    ])
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.updated, vec!["WG-1".to_owned()]);
    assert_eq!(result.failed.len(), 1);
}

#[test]
fn human_edits_to_tracked_fields_are_rejected_and_sync_may_edit() {
    let (slice, row) = tracked();
    let edit = TaskPatch {
        title: Some("Renamed locally".to_owned()),
        ..empty_patch()
    };
    let err = update_plan(&stamp_for("human:brice"), &slice, &row, &edit).err();
    assert_eq!(err.map(|e| e.code), Some(ErrorCode::PermissionDenied));

    let plan = update_plan(&stamp_for("system:tracker-sync"), &slice, &row, &edit)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(plan.task.title, "Renamed locally");
}

#[test]
fn unchanged_owned_values_pass_for_humans() {
    let (slice, row) = tracked();
    let patch = TaskPatch {
        title: Some(row.title.clone()),
        add_tags: vec!["triage".to_owned()],
        ..empty_patch()
    };
    let plan = update_plan(&stamp_for("human:brice"), &slice, &row, &patch)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(plan.tags, Some(vec!["triage".to_owned()]));
}

#[test]
fn human_cannot_cancel_or_archive_a_tracked_task() {
    let (slice, row) = tracked();
    let cancel = TaskPatch {
        status: Some(TaskStatus::Cancelled),
        ..empty_patch()
    };
    let err = update_plan(&stamp_for("human:brice"), &slice, &row, &cancel).err();
    assert_eq!(err.map(|e| e.code), Some(ErrorCode::PermissionDenied));

    let err = archive_plan(&stamp_for("human:brice"), &slice, &row, true).err();
    assert_eq!(err.map(|e| e.code), Some(ErrorCode::PermissionDenied));
    let archived = archive_plan(&stamp_for("system:tracker-sync"), &slice, &row, true)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(archived.change, ArchiveChange::Archived);
    assert_eq!(archived.task.archived_at, Some(stamp(100)));
}

#[test]
fn claim_status_is_refused_by_update() {
    let slice = tasks_slice(vec![task("WG", 1, TaskStatus::Ready, None)], vec![], vec![]);
    let row = slice.task(id(1)).unwrap_or_else(|| panic!("task 1"));
    let patch = TaskPatch {
        status: Some(TaskStatus::Claimed),
        ..empty_patch()
    };
    let err = update_plan(&stamp_for("human:brice"), &slice, row, &patch).err();
    assert_eq!(err.map(|e| e.code), Some(ErrorCode::ValidationFailed));
}

#[test]
fn create_uses_the_workspace_number_and_dedupes_tags() {
    let stamp = stamp_for("human:brice");
    let new = NewTask {
        key: None,
        title: "First".to_owned(),
        description: "Hello".to_owned(),
        acceptance_criteria: vec![],
        status: TaskStatus::Ready,
        priority: TaskPriority::Medium,
        parent_id: None,
        assignee: None,
        assigned_agent: None,
        auto_execute: false,
        due_at: None,
        scheduled_at: None,
        estimated_effort: None,
        metadata: serde_json::Map::new(),
        tags: vec!["a".to_owned(), "b".to_owned(), "a".to_owned()],
        files: vec![],
        projects: vec![],
        repositories: vec![],
        depends_on: vec![],
    };
    let allocation = Allocation {
        workspace_id: common::workspace(),
        key_prefix: "WG",
        number: 1,
        key_taken: false,
    };
    let plan = create_plan(&stamp, allocation, new).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(plan.task.key.to_string(), "WG-1");
    assert_eq!(plan.task.status, TaskStatus::Backlog);
    assert!(plan.ready);
    assert_eq!(plan.tags.len(), 2);
    assert_eq!(
        plan.event.tags,
        VectorSync::from_iter(["a".to_owned(), "b".to_owned()])
    );
}

#[test]
fn only_the_sync_may_name_a_task_with_a_tracker_key() {
    let stamp = stamp_for("human:brice");
    let new = NewTask {
        key: Some("ABC-9".to_owned()),
        title: "x".to_owned(),
        description: String::new(),
        acceptance_criteria: vec![],
        status: TaskStatus::Backlog,
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
    };
    let allocation = Allocation {
        workspace_id: common::workspace(),
        key_prefix: "WG",
        number: 2,
        key_taken: false,
    };
    let err = create_plan(&stamp, allocation, new.clone()).err();
    assert_eq!(err.map(|e| e.code), Some(ErrorCode::PermissionDenied));
    let plan = create_plan(&stamp_for("system:tracker-sync"), allocation, new)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(plan.task.key.to_string(), "ABC-9");
}
