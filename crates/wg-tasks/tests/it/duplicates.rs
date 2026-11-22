//! The duplicate check of a task create: the guard setting, the similarity score, the candidate
//! search with its exclusions, and the refusal with its override.

use crate::common::{id, stamp, task, tasks_slice};
use serde_json::json;
use wg_model::entity::{Task, TaskProject, TaskRepository};
use wg_model::{Row, TaskSlice, Write};
use wg_tasks::duplicates::{
    DuplicateMode, duplicate_guard, duplicates_warning, find_duplicates, refuse_duplicates,
    similarity,
};
use wg_types::{ErrorCode, Id, TaskStatus};

const REPO_A: u128 = 0xA000;
const REPO_B: u128 = 0xB000;
const PROJECT: u128 = 0xC000;

fn titled(n: i64, status: TaskStatus, title: &str, description: &str) -> Task {
    let mut row = task("WG", n, status, None);
    row.title = title.to_owned();
    row.description = description.to_owned();
    row
}

fn in_repository(slice: TaskSlice, task_id: Id, repository: u128) -> TaskSlice {
    slice.apply(&Write::Insert(Row::TaskRepository(TaskRepository {
        task_id,
        repository_id: id(repository),
        access: "read_write".to_owned(),
        created_at: stamp(0),
    })))
}

fn in_project(slice: TaskSlice, task_id: Id, project: u128) -> TaskSlice {
    slice.apply(&Write::Insert(Row::TaskProject(TaskProject {
        task_id,
        project_id: id(project),
    })))
}

const TITLE: &str = "Add a duplicate check to task create";
const BODY: &str = "Look for open tasks with a similar title before a task is written";

/// One open task in repository A with the text above.
fn world_with(status: TaskStatus) -> TaskSlice {
    let slice = tasks_slice(vec![titled(1, status, TITLE, BODY)], vec![], vec![]);
    in_repository(slice, id(1), REPO_A)
}

fn search(slice: &TaskSlice, repositories: &[Id], projects: &[Id]) -> Vec<String> {
    find_duplicates(
        slice,
        crate::common::workspace(),
        (TITLE, BODY),
        (repositories, projects),
        0.6,
    )
    .into_iter()
    .map(|candidate| candidate.key)
    .collect()
}

#[test]
fn guard_defaults_to_refuse_and_reads_mode_and_threshold() {
    let default = duplicate_guard(&serde_json::Map::new());
    assert_eq!(default.mode, DuplicateMode::Refuse);
    assert!((default.threshold - 0.6).abs() < 1e-9);

    let read = |value: serde_json::Value| {
        let config = value.as_object().cloned().unwrap_or_default();
        duplicate_guard(&config)
    };
    let warn = read(json!({"tasks": {"duplicate_check": "warn", "duplicate_threshold": 0.8}}));
    assert_eq!(warn.mode, DuplicateMode::Warn);
    assert!((warn.threshold - 0.8).abs() < 1e-9);
    let off = read(json!({"tasks": {"duplicate_check": "off"}}));
    assert_eq!(off.mode, DuplicateMode::Off);
}

#[test]
fn similarity_is_one_for_a_copy_and_low_for_unrelated_text() {
    assert!((similarity((TITLE, BODY), (TITLE, BODY)) - 1.0).abs() < 1e-9);
    let unrelated = similarity(
        (TITLE, BODY),
        ("Render the humanoid skeleton", "bones and joints"),
    );
    assert!(unrelated < 0.2, "{unrelated}");
    // Case, punctuation and a plural do not matter.
    let reworded = similarity(
        (TITLE, BODY),
        ("add a Duplicate-check to task creates!", BODY),
    );
    assert!(reworded > 0.8, "{reworded}");
}

#[test]
fn a_close_open_task_in_the_same_repository_is_found_with_its_details() {
    let slice = world_with(TaskStatus::Ready);
    let found = find_duplicates(
        &slice,
        crate::common::workspace(),
        (TITLE, BODY),
        (&[id(REPO_A)], &[]),
        0.6,
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].key, "WG-1");
    assert_eq!(found[0].title, TITLE);
    assert_eq!(found[0].status, TaskStatus::Ready);
    assert!((found[0].score - 1.0).abs() < 1e-9);
}

#[test]
fn accepted_and_cancelled_tasks_never_count() {
    for status in [TaskStatus::Accepted, TaskStatus::Cancelled] {
        assert!(search(&world_with(status), &[id(REPO_A)], &[]).is_empty());
    }
    assert_eq!(
        search(&world_with(TaskStatus::Backlog), &[id(REPO_A)], &[]).len(),
        1
    );
}

#[test]
fn archived_tasks_never_count() {
    let mut row = titled(1, TaskStatus::Backlog, TITLE, BODY);
    row.archived_at = Some(stamp(5));
    let slice = in_repository(tasks_slice(vec![row], vec![], vec![]), id(1), REPO_A);
    assert!(search(&slice, &[id(REPO_A)], &[]).is_empty());
}

#[test]
fn tasks_in_another_repository_or_project_never_count() {
    let slice = world_with(TaskStatus::Backlog);
    assert!(search(&slice, &[id(REPO_B)], &[]).is_empty());
    assert!(search(&slice, &[], &[id(PROJECT)]).is_empty());
    // A new task with no repository or project is compared only with tasks that have neither.
    assert!(search(&slice, &[], &[]).is_empty());
    // The same project matches.
    let in_a_project = in_project(slice, id(1), PROJECT);
    assert_eq!(search(&in_a_project, &[], &[id(PROJECT)]).len(), 1);
}

#[test]
fn dissimilar_open_tasks_are_not_candidates() {
    let slice = tasks_slice(
        vec![titled(
            1,
            TaskStatus::Ready,
            "Render the humanoid skeleton",
            "bones",
        )],
        vec![],
        vec![],
    );
    let slice = in_repository(slice, id(1), REPO_A);
    assert!(search(&slice, &[id(REPO_A)], &[]).is_empty());
}

#[test]
fn refuse_lists_the_matches_and_a_distinct_reason_lets_the_create_through() {
    let slice = world_with(TaskStatus::Ready);
    let found = find_duplicates(
        &slice,
        crate::common::workspace(),
        (TITLE, BODY),
        (&[id(REPO_A)], &[]),
        0.6,
    );
    let err = refuse_duplicates(&found, None).err();
    let err = err.unwrap_or_else(|| panic!("a duplicate create must be refused"));
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(err.message.contains("WG-1"));
    assert!(err.message.contains(TITLE));
    assert!(err.message.contains("ready"));
    assert!(err.message.contains("1.00"));
    assert!(err.message.contains("distinct_reason"));
    assert_eq!(err.details.get("reason"), Some(&json!("duplicate_task")));
    assert_eq!(
        err.details["candidates"][0],
        json!({"key": "WG-1", "title": TITLE, "status": "ready", "score": 1.0})
    );

    assert!(refuse_duplicates(&found, Some("covers the HTTP route only")).is_ok());
    // A blank reason is no reason.
    assert!(refuse_duplicates(&found, Some("  ")).is_err());
    // No candidates, nothing to refuse.
    assert!(refuse_duplicates(&[], None).is_ok());
}

#[test]
fn warn_text_names_each_match() {
    let slice = world_with(TaskStatus::Ready);
    let found = find_duplicates(
        &slice,
        crate::common::workspace(),
        (TITLE, BODY),
        (&[id(REPO_A)], &[]),
        0.6,
    );
    let warning = duplicates_warning(&found);
    assert!(warning.contains("WG-1") && warning.contains("similarity 1.00"));
}
