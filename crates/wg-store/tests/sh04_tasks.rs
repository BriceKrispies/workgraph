//! Task-family checks (SH-04): task lookups by key and status, the child rows of a task in their
//! order, dependencies and relations read from both ends, and `task_fts` maintenance (the indexed
//! text, including the classification summary, and removal when the task is gone).
//!
//! The database is built from `contracts/schema-0011.sql` (tables, the FTS virtual table and the
//! triggers), as `sh01_store.rs` builds it.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::entity::{
    Classification, Task, TaskComment, TaskDependency, TaskFile, TaskLink, TaskRelation, TaskTag,
    Workspace,
};
use wg_model::{JsonObject, Row, RowKey, Write, derive_id};
use wg_store::{Location, Raw, Statement, Store};
use wg_types::{
    CommentKind, Id, RelationType, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError,
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
    derive_id(&entropy(2), n)
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

fn put(store: &mut Store, row: impl Into<Row>) -> TestResult {
    store.write(|session| session.apply_write(&Write::insert(row)))?;
    Ok(())
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

fn task(n: u32, number: i64, status: TaskStatus, title: &str) -> Task {
    Task {
        id: id(n),
        workspace_id: id(0),
        number,
        key: ok(TaskKey::parse(&format!("WG-{number}"))),
        title: title.into(),
        description: "plain description".into(),
        description_hash: "h".into(),
        acceptance_criteria: VectorSync::new_sync(),
        status,
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

fn comment(n: u32, task_n: u32, second: u8, body: &str) -> TaskComment {
    TaskComment {
        id: id(n),
        task_id: id(task_n),
        attempt_id: None,
        author: "human:brice".into(),
        kind: CommentKind::Comment,
        body: body.into(),
        data: obj(json!({})),
        created_at: at(second),
    }
}

fn fts_matches(store: &mut Store, term: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let statement = Statement {
        sql: "SELECT task_id FROM task_fts WHERE task_fts MATCH ?1 ORDER BY task_id".to_owned(),
        params: vec![Raw::Text(term.to_owned())],
    };
    let rows = store.read(|s| s.rows(&statement))?;
    Ok(rows
        .into_iter()
        .map(|values| match values.first() {
            Some(Raw::Text(text)) => text.clone(),
            other => panic!("unexpected task_id value {other:?}"),
        })
        .collect())
}

#[test]
fn tasks_are_found_by_key_and_listed_in_number_order() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace())?;
    put(&mut store, task(1, 1, TaskStatus::Backlog, "first"))?;
    put(&mut store, task(2, 2, TaskStatus::Ready, "second"))?;
    put(&mut store, task(3, 3, TaskStatus::Backlog, "third"))?;

    let key = ok(TaskKey::parse("WG-2"));
    let found = store
        .read(|s| s.task_by_key(id(0), &key))?
        .ok_or("WG-2 missing")?;
    assert_eq!(found.id, id(2));
    let missing = ok(TaskKey::parse("WG-9"));
    assert!(store.read(|s| s.task_by_key(id(0), &missing))?.is_none());

    let all = store.read(|s| s.tasks_in_workspace(id(0), None))?;
    let numbers: Vec<i64> = all.iter().map(|t| t.number).collect();
    assert_eq!(numbers, vec![1, 2, 3]);

    let backlog = store.read(|s| s.tasks_in_workspace(id(0), Some(TaskStatus::Backlog)))?;
    let ids: Vec<Id> = backlog.iter().map(|t| t.id).collect();
    assert_eq!(ids, vec![id(1), id(3)]);
    Ok(())
}

#[test]
fn child_rows_come_back_in_their_order() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace())?;
    put(&mut store, task(1, 1, TaskStatus::Backlog, "owner"))?;

    // Inserted out of order on purpose.
    for (position, path) in [(1, "b.rs"), (0, "a.rs")] {
        put(
            &mut store,
            TaskFile {
                task_id: id(1),
                position,
                path: path.into(),
                anchor: None,
                note: String::new(),
            },
        )?;
    }
    for tag in ["beta", "alpha"] {
        put(
            &mut store,
            TaskTag {
                task_id: id(1),
                tag: tag.into(),
            },
        )?;
    }
    put(&mut store, comment(10, 1, 3, "later"))?;
    put(&mut store, comment(11, 1, 2, "earlier"))?;
    put(
        &mut store,
        TaskLink {
            id: id(20),
            task_id: id(1),
            kind: "pull_request".into(),
            provider: "github".into(),
            url: "https://github.com/o/r/pull/7".into(),
            normalized_url: "https://github.com/o/r/pull/7".into(),
            repo_full_name: "o/r".into(),
            number: Some(7),
            repository_id: None,
            sources: VectorSync::new_sync(),
            title: "PR 7".into(),
            created_by: "human:brice".into(),
            created_at: at(4),
            updated_at: at(4),
        },
    )?;

    let files = store.read(|s| s.task_files(id(1)))?;
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, vec!["a.rs", "b.rs"]);

    let tags = store.read(|s| s.task_tags(id(1)))?;
    let names: Vec<&str> = tags.iter().map(|t| t.tag.as_str()).collect();
    assert_eq!(names, vec!["alpha", "beta"]);

    let comments = store.read(|s| s.task_comments(id(1)))?;
    let bodies: Vec<&str> = comments.iter().map(|c| c.body.as_str()).collect();
    assert_eq!(bodies, vec!["earlier", "later"]);

    let links = store.read(|s| s.task_links(id(1)))?;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].number, Some(7));
    Ok(())
}

#[test]
fn dependencies_and_relations_are_read_from_both_ends() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace())?;
    for n in 1..=3 {
        put(&mut store, task(n, i64::from(n), TaskStatus::Backlog, "t"))?;
    }
    // Task 2 waits on task 1.
    put(
        &mut store,
        TaskDependency {
            id: id(30),
            task_id: id(2),
            depends_on_id: id(1),
            created_by: "human:brice".into(),
            created_at: at(5),
        },
    )?;
    // Task 1 relates to task 3.
    put(
        &mut store,
        TaskRelation {
            id: id(31),
            source_task_id: id(1),
            target_task_id: id(3),
            relation_type: RelationType::RelatesTo,
            created_by: "human:brice".into(),
            created_at: at(6),
        },
    )?;

    let waits_on = store.read(|s| s.task_dependencies(id(2)))?;
    assert_eq!(waits_on.len(), 1);
    assert_eq!(waits_on[0].depends_on_id, id(1));
    assert!(store.read(|s| s.task_dependencies(id(1)))?.is_empty());

    let dependents = store.read(|s| s.task_dependents(id(1)))?;
    assert_eq!(dependents.len(), 1);
    assert_eq!(dependents[0].task_id, id(2));
    assert!(store.read(|s| s.task_dependents(id(2)))?.is_empty());

    // The relation shows at both ends and nowhere else.
    assert_eq!(store.read(|s| s.task_relations(id(1)))?.len(), 1);
    assert_eq!(store.read(|s| s.task_relations(id(3)))?.len(), 1);
    assert!(store.read(|s| s.task_relations(id(2)))?.is_empty());
    Ok(())
}

#[test]
fn index_task_makes_the_task_searchable_and_follows_its_children() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace())?;
    let mut indexed = task(1, 1, TaskStatus::Backlog, "Zebra migration");
    indexed.description = "moves the ledger".into();
    put(&mut store, indexed)?;
    put(&mut store, comment(10, 1, 2, "lantern check"))?;
    put(
        &mut store,
        TaskTag {
            task_id: id(1),
            tag: "gold".into(),
        },
    )?;

    assert!(fts_matches(&mut store, "zebra")?.is_empty());
    store.write(|s| s.index_task(id(1)))?;
    assert_eq!(fts_matches(&mut store, "zebra")?, vec![id(1).to_string()]);
    assert_eq!(fts_matches(&mut store, "lantern")?, vec![id(1).to_string()]);
    assert_eq!(fts_matches(&mut store, "gold")?, vec![id(1).to_string()]);
    assert_eq!(fts_matches(&mut store, "ledger")?, vec![id(1).to_string()]);

    // A later comment shows up after a re-index, and the task keeps exactly one index row.
    put(&mut store, comment(11, 1, 9, "heron follow up"))?;
    store.write(|s| s.index_task(id(1)))?;
    assert_eq!(fts_matches(&mut store, "heron")?, vec![id(1).to_string()]);
    let rows = store.read(|s| {
        s.rows(&Statement {
            sql: "SELECT COUNT(*) FROM task_fts WHERE task_id = ?1".to_owned(),
            params: vec![Raw::Text(id(1).to_string())],
        })
    })?;
    assert_eq!(rows, vec![vec![Raw::Int(1)]]);
    Ok(())
}

#[test]
fn the_index_summary_holds_the_current_classification() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace())?;
    let mut classified = task(1, 1, TaskStatus::Backlog, "plain title");
    classified.current_classification_id = Some(id(40));
    put(&mut store, classified)?;
    put(
        &mut store,
        Classification {
            id: id(40),
            task_id: id(1),
            source: "model".into(),
            status: "completed".into(),
            provider: None,
            model: None,
            prompt_version: None,
            taxonomy_version: "1".into(),
            input_hash: "h".into(),
            category: Some("billing".into()),
            subtype: None,
            domain: Some("ledger".into()),
            affected_systems: VectorSync::new_sync().push_back("invoicer".to_owned()),
            complexity: None,
            tags: VectorSync::new_sync().push_back("reconcile".to_owned()),
            confidence: None,
            rationale: None,
            raw_output: None,
            validation_status: "valid".into(),
            error: None,
            duration_ms: None,
            is_current: true,
            confirmed_by: None,
            batch_id: None,
            created_by: "system".into(),
            created_at: at(3),
        },
    )?;

    store.write(|s| s.index_task(id(1)))?;
    for term in ["billing", "ledger", "invoicer", "reconcile"] {
        assert_eq!(
            fts_matches(&mut store, term)?,
            vec![id(1).to_string()],
            "{term}"
        );
    }
    Ok(())
}

#[test]
fn reindexing_a_task_that_no_longer_exists_removes_its_row() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace())?;
    put(&mut store, task(1, 1, TaskStatus::Backlog, "vanishing"))?;
    store.write(|s| s.index_task(id(1)))?;
    assert_eq!(fts_matches(&mut store, "vanishing")?.len(), 1);

    store.write(|s| s.apply_write(&Write::Delete(RowKey::Task(id(1)))))?;
    store.write(|s| s.index_task(id(1)))?;
    assert!(fts_matches(&mut store, "vanishing")?.is_empty());
    Ok(())
}
