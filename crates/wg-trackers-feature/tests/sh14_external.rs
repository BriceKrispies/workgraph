//! Experiment, predictor, external item and analytics source reads (SH-14) against real SQLite.
//!
//! Rows are written through the generic write path, then read back through the lookups and lists.
//! The database is built from `contracts/schema-0011.sql`, as the SH-04 to SH-07 checks do.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::entity::{ExternalItem, Repository, Task, TaskLink, Workspace};
use wg_model::{JsonObject, Row, Write, derive_id};
use wg_store::{Location, Store};
use wg_trackers_feature::service::reads;
use wg_types::{AccessMode, ErrorCode, Id, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError};

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
    derive_id(&entropy(14), n)
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

fn external(n: u32, task: u32, connection: &str, external_id: &str, missing: bool) -> ExternalItem {
    ExternalItem {
        id: id(n),
        workspace_id: id(0),
        task_id: id(task),
        connection: connection.into(),
        external_id: external_id.into(),
        external_key: format!("PROJ-{external_id}"),
        url: format!("https://tracker.example/{external_id}"),
        tracker_status: "In Progress".into(),
        tracker_status_category: "indeterminate".into(),
        tracker_resolution: None,
        remote_updated_at: None,
        snapshot: obj(json!({})),
        snapshot_hash: "digest".into(),
        synced_at: None,
        missing_since: missing.then(|| at(9)),
        created_at: at(3),
        updated_at: at(3),
    }
}

fn pull_request(n: u32, task: u32, url: &str, repo: &str, repository: Option<u32>) -> TaskLink {
    TaskLink {
        id: id(n),
        task_id: id(task),
        kind: "pull_request".into(),
        provider: "github".into(),
        url: url.into(),
        normalized_url: url.into(),
        repo_full_name: repo.into(),
        number: Some(1),
        repository_id: repository.map(id),
        sources: VectorSync::new_sync(),
        title: "a pull request".into(),
        created_by: "human:brice".into(),
        created_at: at(4),
        updated_at: at(4),
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

#[test]
fn external_items_by_task_connection_and_counts() -> TestResult {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            Row::Workspace(workspace()),
            Row::Task(task(1, 1)),
            Row::Task(task(2, 2)),
            Row::Task(task(3, 3)),
            // Written out of external-id order, one of them missing from the tracker.
            Row::ExternalItem(external(80, 1, "jira", "10002", false)),
            Row::ExternalItem(external(81, 2, "jira", "10001", true)),
            Row::ExternalItem(external(82, 3, "other", "10001", false)),
        ],
    )?;

    store.read(|session| {
        let by_task = reads::external_item_by_task(session, id(2))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "item for task 2 missing"))?;
        assert_eq!(by_task.external_id, "10001");
        assert!(reads::external_item_by_task(session, id(9))?.is_none());

        let by_external = reads::external_item_by_external_id(session, "jira", "10002")?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "item 10002 missing"))?;
        assert_eq!(by_external.task_id, id(1));
        // The same tracker id in another connection is a different item.
        assert!(reads::external_item_by_external_id(session, "jira", "10003")?.is_none());
        let other = reads::external_item_by_external_id(session, "other", "10001")?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "other item missing"))?;
        assert_eq!(other.task_id, id(3));

        let ids: Vec<String> = reads::external_items_of_connection(session, "jira")?
            .into_iter()
            .map(|item| item.external_id)
            .collect();
        assert_eq!(ids, vec!["10001".to_owned(), "10002".to_owned()]);

        assert_eq!(reads::tracked_count(session, "jira")?, 2);
        assert_eq!(reads::tracked_count(session, "other")?, 1);
        assert_eq!(reads::tracked_count(session, "nowhere")?, 0);
        assert_eq!(reads::missing_count(session, "jira")?, 1);
        assert_eq!(reads::missing_count(session, "other")?, 0);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn unresolved_pull_request_repos_join_tracked_tasks_to_their_links() -> TestResult {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            Row::Workspace(workspace()),
            Row::Repository(repository(20, "alpha")),
            Row::Task(task(1, 1)),
            Row::Task(task(2, 2)),
            Row::Task(task(3, 3)),
            Row::ExternalItem(external(80, 1, "jira", "10001", false)),
            Row::ExternalItem(external(81, 2, "jira", "10002", false)),
            // Untracked task 3 (other connection) must not contribute its link.
            Row::ExternalItem(external(82, 3, "other", "10003", false)),
            Row::TaskLink(pull_request(
                90,
                1,
                "https://github.example/org/zeta/pull/1",
                "org/zeta",
                None,
            )),
            Row::TaskLink(pull_request(
                91,
                2,
                "https://github.example/org/alpha/pull/2",
                "org/alpha",
                None,
            )),
            Row::TaskLink(pull_request(
                92,
                2,
                "https://github.example/org/alpha/pull/3",
                "org/alpha",
                None,
            )),
            // Resolved to a repository row, so not unresolved.
            Row::TaskLink(pull_request(
                93,
                1,
                "https://github.example/org/resolved/pull/4",
                "org/resolved",
                Some(20),
            )),
            Row::TaskLink(pull_request(
                94,
                3,
                "https://github.example/org/untracked/pull/5",
                "org/untracked",
                None,
            )),
        ],
    )?;

    store.read(|session| {
        let names = reads::unresolved_pull_request_repos(session, "jira")?;
        assert_eq!(names, vec!["org/alpha".to_owned(), "org/zeta".to_owned()]);
        assert!(reads::unresolved_pull_request_repos(session, "nowhere")?.is_empty());
        Ok(())
    })?;
    Ok(())
}
