//! Stack family reads (SH-07) against real SQLite: stacks, stack repositories and stack items
//! written through the generic write path come back through the lookups and lists, with the
//! active/inactive filters and the merge-order and name orderings the Python services use.
//!
//! The database is built from `contracts/schema-0011.sql`, as the SH-04 and SH-05 checks do.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::entity::{Repository, Stack, StackItem, StackRepository, Task, Workspace};
use wg_model::{JsonObject, Row, Write, derive_id};
use wg_stacks_feature::service::reads;
use wg_store::{Location, Store};
use wg_types::{
    AccessMode, ErrorCode, Id, StackItemState, StackStatus, TaskKey, TaskPriority, TaskStatus,
    Timestamp, WgError,
};

type TestResult = Result<(), Box<dyn Error>>;

/// Unwraps a fixture value that is valid by construction.
fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn schema() -> Result<String, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/schema-0011.sql");
    let text = std::fs::read_to_string(path)?;
    let wanted: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|block| {
            (block.starts_with("CREATE TABLE") && !block.starts_with("CREATE TABLE 'task_fts_"))
                || block.starts_with("CREATE VIRTUAL TABLE")
                || block.starts_with("CREATE TRIGGER")
        })
        .collect();
    Ok(wanted.join(";\n"))
}

fn store() -> Result<Store, Box<dyn Error>> {
    let mut store = Store::open(&Location::Memory)?;
    let ddl = schema()?;
    store.write(|session| session.execute_batch(&ddl))?;
    Ok(store)
}

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn id(n: u32) -> Id {
    derive_id(&entropy(7), n)
}

fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 25, 12, 0, second, 0))
}

fn obj(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

fn workspace() -> Workspace {
    Workspace {
        id: id(0),
        slug: "default".into(),
        name: "Default".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 10,
        settings: obj(json!({})),
        archived_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

fn repository(n: u32, name: &str) -> Repository {
    Repository {
        id: id(n),
        workspace_id: id(0),
        name: name.into(),
        description: String::new(),
        identity_fingerprint: None,
        default_branch: Some("main".into()),
        tracked_branches: VectorSync::new_sync(),
        access_mode: AccessMode::ReadWrite,
        health_status: "ok".into(),
        health_detail: Default::default(),
        health_checked_at: None,
        is_shallow: false,
        last_scanned_at: None,
        archived_at: None,
        metadata: Default::default(),
        created_at: at(2),
        updated_at: at(2),
    }
}

fn task(n: u32, number: i64) -> Task {
    Task {
        id: id(n),
        workspace_id: id(0),
        number,
        key: ok(TaskKey::parse(&format!("WG-{number}"))),
        title: format!("task {number}"),
        description: "plain description".into(),
        description_hash: "h".into(),
        acceptance_criteria: VectorSync::new_sync(),
        status: TaskStatus::Ready,
        priority: TaskPriority::Medium,
        category: None,
        parent_id: None,
        assignee: None,
        assigned_agent: None,
        claimed_by: None,
        claim_expires_at: None,
        auto_execute: false,
        estimated_effort: None,
        metadata: obj(json!({})),
        due_at: None,
        scheduled_at: None,
        created_by: "human:brice".into(),
        version: 1,
        created_at: at(1),
        updated_at: at(1),
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    }
}

fn stack(n: u32, parent: u32, status: StackStatus, created: u8) -> Stack {
    Stack {
        id: id(n),
        workspace_id: id(0),
        parent_task_id: id(parent),
        status,
        created_at: at(created),
        updated_at: at(created),
        land_request: None,
    }
}

fn stack_item(n: u32, stack: u32, task: u32, merge_order: Option<i64>) -> StackItem {
    StackItem {
        id: id(n),
        stack_id: id(stack),
        task_id: id(task),
        state: StackItemState::Pending,
        merge_order,
        detail: None,
        created_at: at(5),
        updated_at: at(5),
    }
}

fn stack_repository(stack: u32, repository: u32, base: &str) -> StackRepository {
    StackRepository {
        stack_id: id(stack),
        repository_id: id(repository),
        base_sha: base.into(),
        integration_branch: "integration".into(),
        created_at: at(4),
    }
}

/// Inserts every row in order, one write each, through the generic write path.
fn insert_all(store: &mut Store, rows: Vec<Row>) -> TestResult {
    store.write(|session| {
        for row in rows {
            session.apply_write(&Write::Insert(row))?;
        }
        Ok(())
    })?;
    Ok(())
}

/// The fixture: one parent task (1) with two stacks. Stack 30 has landed (newer), stack 31 is
/// open (older). Stack 31 holds tasks 2, 3 and 4 (merge orders 2, 1 and none); stack 30 holds
/// task 5 and covers repository 21. Stack 31 covers repositories 20 ("beta") and 21 ("alpha").
fn fixture(store: &mut Store) -> TestResult {
    insert_all(
        store,
        vec![
            Row::Workspace(workspace()),
            Row::Repository(repository(20, "beta")),
            Row::Repository(repository(21, "alpha")),
            Row::Task(task(1, 1)),
            Row::Task(task(2, 2)),
            Row::Task(task(3, 3)),
            Row::Task(task(4, 4)),
            Row::Task(task(5, 5)),
            Row::Stack(stack(31, 1, StackStatus::Open, 4)),
            Row::Stack(stack(30, 1, StackStatus::Landed, 9)),
            Row::StackItem(stack_item(40, 31, 2, Some(2))),
            Row::StackItem(stack_item(41, 31, 3, Some(1))),
            Row::StackItem(stack_item(42, 31, 4, None)),
            Row::StackItem(stack_item(43, 30, 5, None)),
            Row::StackRepository(stack_repository(31, 20, "aaa111")),
            Row::StackRepository(stack_repository(31, 21, "bbb222")),
            Row::StackRepository(stack_repository(30, 21, "ccc333")),
        ],
    )
}

#[test]
fn stack_lookups_pick_the_active_and_the_latest() -> TestResult {
    let mut store = store()?;
    fixture(&mut store)?;

    store.read(|session| {
        let by_id = session
            .stack_by_id(id(31))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "stack 31 missing"))?;
        assert_eq!(by_id.status, StackStatus::Open);
        assert!(session.stack_by_id(id(99))?.is_none());

        let active = reads::active_stack_for_parent(session, id(1))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "no active stack"))?;
        assert_eq!(active.id, id(31));
        assert!(reads::active_stack_for_parent(session, id(2))?.is_none());

        // The landed stack is newer, so it is the latest one even though it is not active.
        let latest = reads::latest_stack_for_parent(session, id(1))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "no latest stack"))?;
        assert_eq!(latest.id, id(30));

        let open_only = reads::stacks_in_workspace(session, id(0), false)?;
        let open_ids: Vec<Id> = open_only.iter().map(|s| s.id).collect();
        assert_eq!(open_ids, vec![id(31)]);
        let all = reads::stacks_in_workspace(session, id(0), true)?;
        let all_ids: Vec<Id> = all.iter().map(|s| s.id).collect();
        assert_eq!(all_ids, vec![id(30), id(31)]);

        let containing = reads::stacks_containing_task(session, id(5))?;
        assert_eq!(containing.len(), 1);
        assert_eq!(containing[0].id, id(30));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn stack_items_come_in_merge_order() -> TestResult {
    let mut store = store()?;
    fixture(&mut store)?;

    store.read(|session| {
        let items = reads::stack_items(session, id(31))?;
        let tasks: Vec<Id> = items.iter().map(|item| item.task_id).collect();
        // Merge order 1 (task 3), merge order 2 (task 2), then the unordered item (task 4).
        assert_eq!(tasks, vec![id(3), id(2), id(4)]);

        let one = reads::stack_item(session, id(31), id(2))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "item missing"))?;
        assert_eq!(one.id, id(40));
        assert_eq!(one.merge_order, Some(2));
        assert!(reads::stack_item(session, id(31), id(5))?.is_none());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn task_items_respect_the_active_stack() -> TestResult {
    let mut store = store()?;
    fixture(&mut store)?;

    store.read(|session| {
        let active = reads::active_item_for_task(session, id(2))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "task 2 has no active item"))?;
        assert_eq!(active.stack_id, id(31));
        // Task 5 is only in the landed stack, so it has no active item but one item overall.
        assert!(reads::active_item_for_task(session, id(5))?.is_none());

        let across = reads::items_for_task(session, id(5))?;
        assert_eq!(across.len(), 1);
        assert_eq!(across[0].stack_id, id(30));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn stack_repositories_by_name_and_by_task() -> TestResult {
    let mut store = store()?;
    fixture(&mut store)?;

    store.read(|session| {
        let repos = reads::stack_repositories(session, id(31))?;
        let order: Vec<Id> = repos.iter().map(|r| r.repository_id).collect();
        // Repository 21 is named "alpha" and repository 20 "beta".
        assert_eq!(order, vec![id(21), id(20)]);

        let covered = reads::stack_repository(session, id(31), id(20))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "repository 20 not covered"))?;
        assert_eq!(covered.base_sha, "aaa111");
        assert!(reads::stack_repository(session, id(31), id(99))?.is_none());

        // Task 2 is in the active stack 31, which covers repository 21.
        let base = session
            .active_repository_for_task(id(2), id(21))?
            .ok_or_else(|| {
                WgError::new(ErrorCode::NotFound, "no active repository row for task 2")
            })?;
        assert_eq!(base.base_sha, "bbb222");
        assert_eq!(base.integration_branch, "integration");
        // Task 5 is only in the landed stack 30, which is not active.
        assert!(session.active_repository_for_task(id(5), id(21))?.is_none());
        Ok(())
    })?;
    Ok(())
}
