//! Experiment, predictor, external item and analytics source reads (SH-14) against real SQLite.
//!
//! Rows are written through the generic write path, then read back through the lookups and lists.
//! The database is built from `contracts/schema-0011.sql`, as the SH-04 to SH-07 checks do.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_analytics_feature::service::reads;
use wg_analytics_feature::spec::sql::{self, TABLES};
use wg_model::entity::{Task, Workspace};
use wg_model::{JsonObject, Row, Write, derive_id};
use wg_rows::Raw;
use wg_store::{Location, Store};
use wg_types::{ErrorCode, Id, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError};

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
fn analytics_sources_copy_the_wanted_columns_in_order() -> TestResult {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            Row::Workspace(workspace()),
            Row::Task(task(1, 1)),
            Row::Task(task(2, 2)),
        ],
    )?;

    store.read(|session| {
        let tasks = reads::analytics_source(session, "tasks")?;
        let wanted = sql::wanted_columns("tasks")
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "tasks not listed"))?;
        assert_eq!(tasks.columns, wanted.to_vec());
        assert_eq!(tasks.rows.len(), 2);
        // Rows come back in rowid (insertion) order: task 1 first.
        assert_eq!(tasks.rows[0][0], Raw::Text(id(1).to_string()));

        assert!(reads::analytics_source(session, "not_a_table").is_err());

        let all = reads::analytics_sources(session)?;
        let names: Vec<&str> = all.iter().map(|source| source.table.as_str()).collect();
        let listed: Vec<&str> = TABLES.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, listed);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn source_columns_keep_only_present_columns_in_wanted_order() {
    let available: Vec<String> = ["id", "name", "status"]
        .iter()
        .map(|c| (*c).to_owned())
        .collect();
    assert_eq!(
        sql::source_columns(Some(&["status", "gone", "id"]), &available),
        vec!["status".to_owned(), "id".to_owned()]
    );
    assert_eq!(sql::source_columns(None, &available), available);
}
