//! wg-store base checks (SH-01): events appended in the Python-written form, the immutability
//! triggers, decisions applied atomically, and a row codec round trip for every model table.
//!
//! The database is built from `contracts/schema-0011.sql` (tables, the FTS virtual table and the
//! triggers; indexes and the FTS shadow tables are not needed here).

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::entity::{
    ExternalItem, Project, ProjectRepository, Repository, RepositoryCheckout, RepositoryDependency,
    RepositoryRef, RepositoryRemote, Task, TaskComment, TaskDependency, TaskFile, TaskLink,
    TaskProject, TaskRelation, TaskRepository, TaskTag, Workspace,
};
use wg_model::event::{TaskCreated, TaskKeyed};
use wg_model::{
    Decision, EventDraft, EventPayload, JsonObject, Row, RowKey, Stamp, Table, Write, derive_id,
};
use wg_rows::columns;
use wg_store::{EventQuery, Location, Raw, Statement, Store};
use wg_types::{
    AccessMode, Actor, CommentKind, ErrorCode, Id, RelationType, TaskKey, TaskPriority, TaskStatus,
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
    derive_id(&entropy(1), n)
}

fn at(second: u8, micros: u32) -> Timestamp {
    ok(Timestamp::from_components(
        2026, 10, 25, 12, 0, second, micros,
    ))
}

fn stamp(actor: &str, automation_depth: u32) -> Stamp {
    Stamp {
        at: at(5, 123_456),
        entropy: entropy(7),
        actor: ok(Actor::parse(actor)),
        permissions: Default::default(),
        correlation: ok(wg_types::parse_id("33333333-3333-4333-8333-333333333333")).into(),
        causation: None,
        attempt: None,
        automation_depth,
    }
}

fn workspace() -> Workspace {
    Workspace {
        id: id(0),
        slug: "default".into(),
        name: "Default".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 3,
        settings: obj(json!({"b": 1, "a": [true, null, "é"]})),
        archived_at: None,
        created_at: at(0, 0),
        updated_at: at(0, 5),
    }
}

fn obj(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

fn task(n: u32, number: i64) -> Task {
    Task {
        id: id(n),
        workspace_id: id(0),
        number,
        key: ok(TaskKey::parse(&format!("WG-{number}"))),
        title: format!("Task {number}"),
        description: "d".into(),
        description_hash: "h".into(),
        acceptance_criteria: VectorSync::new_sync().push_back("works".to_owned()),
        status: TaskStatus::Backlog,
        priority: TaskPriority::Medium,
        category: Some("testing".into()),
        parent_id: None,
        assignee: None,
        assigned_agent: Some("a".into()),
        claimed_by: None,
        claim_expires_at: Some(at(9, 1)),
        auto_execute: true,
        estimated_effort: Some(obj(json!({"points": 3, "ratio": 0.5}))),
        metadata: obj(json!({"k": "v"})),
        due_at: None,
        scheduled_at: None,
        created_by: "human:brice".into(),
        version: 1,
        created_at: at(1, 0),
        updated_at: at(1, 0),
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    }
}

fn created_event(stamp: &Stamp, event_id: Id) -> EventDraft {
    let payload = EventPayload::TaskCreated(TaskCreated {
        key: ok(TaskKey::parse("WG-7")),
        title: "Ship it \u{e9}".into(),
        description: "body".into(),
        acceptance_criteria: VectorSync::new_sync().push_back("works".to_owned()),
        status: TaskStatus::Backlog,
        priority: TaskPriority::High,
        tags: VectorSync::new_sync().push_back("a".to_owned()),
        project_ids: VectorSync::new_sync(),
        repository_ids: VectorSync::new_sync(),
        parent_id: None,
        depends_on: VectorSync::new_sync(),
        assignee: Some("brice".into()),
        auto_execute: true,
    });
    let workspace_id = ok(wg_types::parse_id("11111111-1111-4111-8111-111111111111"));
    let entity_id = ok(wg_types::parse_id("22222222-2222-4222-8222-222222222222"));
    EventDraft::new(stamp, event_id, Some(workspace_id), entity_id, payload)
}

type RawEvent = (
    i64,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    i64,
    String,
);

/// The columns of an event row as SQLite stores them, with `id` left out (Python draws it at
/// random) and `schema_version` and `data` kept as text.
fn raw_event(store: &mut Store, seq: i64) -> Result<(RawEvent, String), Box<dyn Error>> {
    let statement = Statement {
        sql: "SELECT seq, event_type, workspace_id, entity_type, actor, actor_kind, occurred_at, \
              schema_version, correlation_id, data FROM events WHERE seq = ?1"
            .to_owned(),
        params: vec![Raw::Int(seq)],
    };
    let rows = store.read(|session| session.rows(&statement))?;
    let row = rows.first().ok_or("no such event")?;
    let text = |index: usize| match &row[index] {
        Raw::Text(text) => text.clone(),
        other => panic!("column {index} is {other:?}"),
    };
    let workspace = match &row[2] {
        Raw::Text(text) => Some(text.clone()),
        _ => None,
    };
    let (Raw::Int(seq), Raw::Int(version)) = (&row[0], &row[7]) else {
        return Err("integer columns".into());
    };
    Ok((
        (
            *seq,
            text(1),
            workspace,
            text(3),
            text(4),
            text(5),
            text(6),
            *version,
            text(8),
        ),
        text(9),
    ))
}

/// Rows copied from a database that Python's `record_event` wrote (see the task report): an
/// automation-depth event, then a workspace-less one with a whole-second timestamp.
#[test]
fn appended_event_rows_equal_the_python_written_form() -> TestResult {
    let mut store = store()?;
    let first = created_event(&stamp("automation:rule", 1), id(10));
    let second_stamp = stamp("human:brice", 0);
    let second = EventDraft::new(
        &second_stamp,
        id(11),
        None,
        ok(wg_types::parse_id("22222222-2222-4222-8222-222222222222")),
        EventPayload::TaskArchived(TaskKeyed {
            key: ok(TaskKey::parse("WG-7")),
        }),
    );
    let second = EventDraft {
        occurred_at: at(6, 0),
        ..second
    };
    let decision = Decision::new(()).event(first).event(second);
    let applied = store.commit_decision(&decision)?;
    assert_eq!(applied.event_seqs, vec![1, 2]);

    let (columns, data) = raw_event(&mut store, 1)?;
    assert_eq!(
        columns,
        (
            1,
            "task.created".to_owned(),
            Some("11111111-1111-4111-8111-111111111111".to_owned()),
            "task".to_owned(),
            "automation:rule".to_owned(),
            "automation".to_owned(),
            "2026-10-25T12:00:05.123456+00:00".to_owned(),
            2,
            "33333333-3333-4333-8333-333333333333".to_owned(),
        )
    );
    assert_eq!(
        data,
        r#"{"_automation":{"actor":"automation:rule","depth":1},"acceptance_criteria":["works"],"assignee":"brice","auto_execute":true,"depends_on":[],"description":"body","key":"WG-7","parent_id":null,"priority":"high","project_ids":[],"repository_ids":[],"status":"backlog","tags":["a"],"title":"Ship it \u00e9"}"#
    );

    let (columns, data) = raw_event(&mut store, 2)?;
    assert_eq!(
        columns,
        (
            2,
            "task.archived".to_owned(),
            None,
            "task".to_owned(),
            "human:brice".to_owned(),
            "human".to_owned(),
            "2026-10-25T12:00:06.000000+00:00".to_owned(),
            1,
            "33333333-3333-4333-8333-333333333333".to_owned(),
        )
    );
    assert_eq!(data, r#"{"key":"WG-7"}"#);

    // Read back, `to_dict` equals Python's `EventRecord.to_dict()` of the same rows.
    let records = store.read(|s| s.query_events(&EventQuery::default()))?;
    let dicts: Vec<Value> = records.iter().map(|record| record.to_dict()).collect();
    assert_eq!(
        dicts[0],
        json!({
            "actor": "automation:rule", "actor_kind": "automation", "attempt_id": null,
            "causation_id": null, "correlation_id": "33333333-3333-4333-8333-333333333333",
            "data": {
                "_automation": {"actor": "automation:rule", "depth": 1},
                "acceptance_criteria": ["works"], "assignee": "brice", "auto_execute": true,
                "depends_on": [], "description": "body", "key": "WG-7", "parent_id": null,
                "priority": "high", "project_ids": [], "repository_ids": [], "status": "backlog",
                "tags": ["a"], "title": "Ship it \u{e9}"
            },
            "entity_id": "22222222-2222-4222-8222-222222222222", "entity_type": "task",
            "event_type": "task.created", "id": id(10).to_string(),
            "occurred_at": "2026-10-25T12:00:05.123456+00:00", "schema_version": 2, "seq": 1,
            "workspace_id": "11111111-1111-4111-8111-111111111111"
        })
    );
    assert_eq!(dicts[1]["occurred_at"], "2026-10-25T12:00:06+00:00");
    assert_eq!(dicts[1]["workspace_id"], Value::Null);
    assert_eq!(store.read(|s| s.latest_seq())?, 2);
    Ok(())
}

#[test]
fn events_are_read_by_entity_and_by_workspace_after_seq() -> TestResult {
    let mut store = store()?;
    let entity = ok(wg_types::parse_id("22222222-2222-4222-8222-222222222222"));
    let other = id(50);
    let stamp = stamp("human:brice", 0);
    let mut decision = Decision::new(());
    for n in 0..3 {
        decision = decision.event(created_event(&stamp, id(20 + n)));
    }
    let elsewhere = EventDraft::new(
        &stamp,
        id(30),
        Some(id(0)),
        other,
        EventPayload::TaskArchived(TaskKeyed {
            key: ok(TaskKey::parse("WG-1")),
        }),
    );
    store.commit_decision(&decision.event(elsewhere))?;

    let mut seqs = |query: &EventQuery| -> Result<Vec<i64>, WgError> {
        store.read(|s| Ok(s.query_events(query)?.iter().map(|e| e.seq).collect()))
    };
    assert_eq!(seqs(&EventQuery::by_entity("task", entity))?, vec![1, 2, 3]);
    assert_eq!(seqs(&EventQuery::by_entity("task", other))?, vec![4]);
    let ws = ok(wg_types::parse_id("11111111-1111-4111-8111-111111111111"));
    assert_eq!(seqs(&EventQuery::after(ws, 1))?, vec![2, 3]);
    assert_eq!(seqs(&EventQuery::after(id(0), 0))?, vec![4]);
    let newest = EventQuery {
        descending: true,
        limit: 2,
        ..EventQuery::default()
    };
    assert_eq!(seqs(&newest)?, vec![4, 3]);
    let prefixed = EventQuery {
        event_type_prefix: Some("task.arch".into()),
        ..EventQuery::default()
    };
    assert_eq!(seqs(&prefixed)?, vec![4]);
    Ok(())
}

#[test]
fn old_task_created_payloads_are_upcast_on_read() -> TestResult {
    let mut store = store()?;
    let insert = Statement {
        sql: "INSERT INTO events (id, workspace_id, event_type, entity_type, entity_id, actor, \
              actor_kind, occurred_at, correlation_id, schema_version, data) \
              VALUES (?1, NULL, 'task.created', 'task', ?1, 'human:b', 'human', \
              '2026-10-25T12:00:00.000000+00:00', ?1, 1, '{\"tags\":\" a, b ,,c\"}')"
            .to_owned(),
        params: vec![Raw::Text(id(1).to_string())],
    };
    store.write(|s| s.run(&insert))?;
    let records = store.read(|s| s.query_events(&EventQuery::default()))?;
    assert_eq!(records[0].schema_version, 2);
    assert_eq!(
        Value::Object(records[0].data.clone()),
        json!({"tags": ["a", "b", "c"], "acceptance_criteria": []})
    );
    Ok(())
}

#[test]
fn events_reject_update_and_delete() -> TestResult {
    let mut store = store()?;
    store.commit_decision(&Decision::new(()).event(created_event(&stamp("human:b", 0), id(10))))?;
    for sql in ["UPDATE events SET actor = 'x'", "DELETE FROM events"] {
        let statement = Statement {
            sql: sql.to_owned(),
            params: Vec::new(),
        };
        let err = store.write(|s| s.run(&statement)).err();
        let err = err.ok_or("trigger did not fire")?;
        assert_eq!(err.code, ErrorCode::Conflict, "{sql}");
        assert!(err.message.contains("append-only"), "{err}");
    }
    assert_eq!(store.read(|s| s.latest_seq())?, 1);
    Ok(())
}

#[test]
fn a_failing_write_rolls_back_the_whole_decision() -> TestResult {
    let mut store = store()?;
    let ws = workspace();
    store.commit_decision(&Decision::new(()).write(Write::insert(ws.clone())))?;

    // A new task, its event, then a duplicate workspace slug: nothing may persist.
    let clash = Workspace {
        id: id(99),
        ..ws.clone()
    };
    let decision = Decision::new(())
        .write(Write::insert(task(1, 1)))
        .event(created_event(&stamp("human:b", 0), id(10)))
        .write(Write::insert(clash));
    let err = store.commit_decision(&decision).err().ok_or("no error")?;
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(
        store
            .read(|s| s.fetch::<Task>(&RowKey::Task(id(1))))?
            .is_none()
    );
    assert_eq!(store.read(|s| s.latest_seq())?, 0);
    assert!(
        store
            .read(|s| s.fetch::<Workspace>(&RowKey::Workspace(id(0))))?
            .is_some()
    );
    Ok(())
}

#[test]
fn versioned_updates_detect_concurrent_changes() -> TestResult {
    let mut store = store()?;
    let first = task(1, 1);
    store.commit_decision(
        &Decision::new(())
            .write(Write::insert(workspace()))
            .write(Write::insert(first.clone())),
    )?;
    let next = Task {
        title: "renamed".into(),
        status: TaskStatus::Ready,
        version: 2,
        ..first.clone()
    };
    store.commit_decision(&Decision::new(()).write(Write::update_versioned(next.clone(), 1)))?;
    let read = store.read(|s| s.fetch::<Task>(&RowKey::Task(id(1))))?;
    assert_eq!(read, Some(next.clone()));

    // The stored row is now at version 2; an update guarded by 1 fails and changes nothing.
    let stale = Task {
        title: "lost".into(),
        version: 2,
        ..first
    };
    let err = store
        .commit_decision(&Decision::new(()).write(Write::update_versioned(stale, 1)))
        .err()
        .ok_or("no error")?;
    assert_eq!(err.code, ErrorCode::VersionConflict);
    assert_eq!(err.details.get("expected"), Some(&json!(1)));
    assert_eq!(err.details.get("actual"), Some(&json!(2)));
    let read = store.read(|s| s.fetch::<Task>(&RowKey::Task(id(1))))?;
    assert_eq!(read, Some(next));

    // Updating a row that does not exist is not_found; deleting one is a no-op.
    let err = store
        .commit_decision(&Decision::new(()).write(Write::update(task(5, 5))))
        .err()
        .ok_or("no error")?;
    assert_eq!(err.code, ErrorCode::NotFound);
    store.commit_decision(&Decision::new(()).write(Write::delete(RowKey::Task(id(5)))))?;
    Ok(())
}

/// Every model table: insert, read back equal, update, read back equal, delete.
#[test]
fn every_table_round_trips() -> TestResult {
    let mut store = store()?;
    let (t1, t2) = (task(1, 1), task(2, 2));
    let repo = |n: u32, name: &str| Repository {
        id: id(n),
        workspace_id: id(0),
        name: name.into(),
        description: "r".into(),
        identity_fingerprint: Some("fp".into()),
        default_branch: Some("main".into()),
        tracked_branches: VectorSync::new_sync().push_back("main".to_owned()),
        access_mode: AccessMode::ReadWrite,
        health_status: "ok".into(),
        health_detail: obj(json!({"x": 1})),
        health_checked_at: Some(at(2, 7)),
        is_shallow: true,
        last_scanned_at: None,
        archived_at: None,
        metadata: JsonObject::new(),
        created_at: at(2, 0),
        updated_at: at(2, 0),
    };
    let rows: Vec<Row> = vec![
        workspace().into(),
        Project {
            id: id(3),
            workspace_id: id(0),
            slug: "p".into(),
            name: "P".into(),
            description: String::new(),
            status: "active".into(),
            settings: obj(json!({"s": [1, 2]})),
            created_at: at(2, 0),
            updated_at: at(2, 0),
        }
        .into(),
        repo(4, "r1").into(),
        repo(5, "r2").into(),
        ProjectRepository {
            project_id: id(3),
            repository_id: id(4),
            role: "primary".into(),
            access_mode: Some("read_only".into()),
            created_at: at(2, 0),
        }
        .into(),
        RepositoryCheckout {
            id: id(6),
            repository_id: id(4),
            path: "/x".into(),
            kind: "main".into(),
            is_primary: true,
            access_mode: None,
            available: false,
            last_seen_at: Some(at(3, 0)),
            created_at: at(2, 0),
            updated_at: at(2, 0),
        }
        .into(),
        RepositoryRemote {
            id: id(7),
            repository_id: id(4),
            name: "origin".into(),
            url: "u".into(),
            normalized_url: "n".into(),
            created_at: at(2, 0),
            updated_at: at(2, 0),
        }
        .into(),
        RepositoryRef {
            id: id(8),
            repository_id: id(4),
            name: "main".into(),
            sha: "abc".into(),
            updated_at: at(2, 0),
        }
        .into(),
        RepositoryDependency {
            id: id(9),
            from_repository_id: id(4),
            to_repository_id: id(5),
            kind: "uses".into(),
            description: String::new(),
            created_at: at(2, 0),
        }
        .into(),
        t1.clone().into(),
        t2.clone().into(),
        TaskFile {
            task_id: id(1),
            position: 0,
            path: "a.rs".into(),
            anchor: None,
            note: "n".into(),
        }
        .into(),
        TaskTag {
            task_id: id(1),
            tag: "t".into(),
        }
        .into(),
        TaskLink {
            id: id(10),
            task_id: id(1),
            kind: "pull_request".into(),
            provider: "github".into(),
            url: "u".into(),
            normalized_url: "n".into(),
            repo_full_name: "o/r".into(),
            number: Some(12),
            repository_id: Some(id(4)),
            sources: VectorSync::new_sync().push_back("trailer".to_owned()),
            title: "t".into(),
            created_by: "human:b".into(),
            created_at: at(2, 0),
            updated_at: at(2, 0),
        }
        .into(),
        TaskComment {
            id: id(11),
            task_id: id(1),
            attempt_id: Some(id(77)),
            author: "human:b".into(),
            kind: CommentKind::Progress,
            body: "b".into(),
            data: obj(json!({"pct": 50})),
            created_at: at(2, 0),
        }
        .into(),
        TaskDependency {
            id: id(12),
            task_id: id(1),
            depends_on_id: id(2),
            created_by: "human:b".into(),
            created_at: at(2, 0),
        }
        .into(),
        TaskRelation {
            id: id(13),
            source_task_id: id(1),
            target_task_id: id(2),
            relation_type: RelationType::RelatesTo,
            created_by: "human:b".into(),
            created_at: at(2, 0),
        }
        .into(),
        TaskProject {
            task_id: id(1),
            project_id: id(3),
        }
        .into(),
        TaskRepository {
            task_id: id(1),
            repository_id: id(4),
            access: "read_write".into(),
            created_at: at(2, 0),
        }
        .into(),
        ExternalItem {
            id: id(14),
            workspace_id: id(0),
            task_id: id(1),
            connection: "jira".into(),
            external_id: "10001".into(),
            external_key: "ABC-1".into(),
            url: "u".into(),
            tracker_status: "Open".into(),
            tracker_status_category: "new".into(),
            tracker_resolution: None,
            remote_updated_at: Some(at(4, 0)),
            snapshot: obj(json!({"fields": {"b": 2, "a": 1}})),
            snapshot_hash: "h".into(),
            synced_at: None,
            missing_since: None,
            created_at: at(2, 0),
            updated_at: at(2, 0),
        }
        .into(),
    ];
    let mut tables: Vec<Table> = rows.iter().map(Row::table).collect();
    tables.sort();
    tables.dedup();
    assert_eq!(
        tables,
        Table::ALL[..18].to_vec(),
        "the first 18 tables are covered"
    );

    let mut decision = Decision::new(());
    for row in &rows {
        decision = decision.write(Write::insert(row.clone()));
    }
    store.commit_decision(&decision)?;
    for row in &rows {
        assert_eq!(read_back(&mut store, &row.key())?, Some(row.clone()));
    }

    // Replace every row; the keyed-only tables stay as they are.
    let changed = Task {
        version: 2,
        title: "again".into(),
        estimated_effort: None,
        ..t1
    };
    store.commit_decision(
        &Decision::new(())
            .write(Write::update_versioned(changed.clone(), 1))
            .write(Write::update(TaskTag {
                task_id: id(1),
                tag: "t".into(),
            })),
    )?;
    assert_eq!(
        read_back(&mut store, &RowKey::Task(id(1)))?,
        Some(changed.into())
    );

    // Deleting a task cascades to its children.
    store.commit_decision(&Decision::new(()).write(Write::delete(RowKey::Task(id(1)))))?;
    for key in [
        RowKey::Task(id(1)),
        RowKey::TaskFile {
            task_id: id(1),
            position: 0,
        },
        RowKey::TaskTag {
            task_id: id(1),
            tag: "t".into(),
        },
        RowKey::TaskComment(id(11)),
        RowKey::ExternalItem(id(14)),
    ] {
        assert_eq!(read_back(&mut store, &key)?, None, "{key:?}");
    }
    Ok(())
}

/// Reads one row through its table's codec.
fn read_back(store: &mut Store, key: &RowKey) -> Result<Option<Row>, WgError> {
    store.read(|s| s.fetch_row(key))
}

#[test]
fn json_columns_are_stored_as_python_stores_them() -> TestResult {
    let mut store = store()?;
    store.commit_decision(&Decision::new(()).write(Write::insert(workspace())))?;
    let statement = Statement {
        sql: "SELECT settings, created_at FROM workspaces".to_owned(),
        params: Vec::new(),
    };
    let rows = store.read(|s| s.rows(&statement))?;
    assert_eq!(
        rows,
        vec![vec![
            Raw::Text(r#"{"a":[true,null,"\u00e9"],"b":1}"#.to_owned()),
            Raw::Text("2026-10-25T12:00:00.000000+00:00".to_owned()),
        ]]
    );
    // Every column the codecs name is a real column of its table.
    for table in Table::ALL {
        let names: Vec<String> = columns(*table).iter().map(|c| format!("\"{c}\"")).collect();
        let probe = Statement {
            sql: format!("SELECT {} FROM {} LIMIT 0", names.join(", "), table.name()),
            params: Vec::new(),
        };
        store.read(|s| s.rows(&probe))?;
    }
    Ok(())
}

/// The stack test's Rust half: with `WG_SH01_DB` naming a database that Python migrated to head,
/// writes a workspace, a task and two events into it, for Python's `query_events` and ORM to read.
/// Skipped when the variable is unset.
#[test]
fn writes_into_a_python_migrated_database() -> TestResult {
    let Some(path) = std::env::var_os("WG_SH01_DB") else {
        return Ok(());
    };
    let mut store = Store::open(&Location::File(path.into()))?;
    let decision = Decision::new(())
        .write(Write::insert(workspace()))
        .write(Write::insert(task(1, 1)))
        .event(created_event(&stamp("automation:rule", 1), id(10)))
        .event(created_event(&stamp("human:brice", 0), id(11)));
    store.commit_decision(&decision)?;
    Ok(())
}

/// Representative rows of the execution and knowledge tables: every codec kind they use (float,
/// blob, JSON lists of objects and of values, string keys, composite keys) round-trips. Foreign
/// keys are switched off so each row stands alone.
#[test]
fn execution_and_knowledge_rows_round_trip() -> TestResult {
    use wg_model::entity::{
        Commit, JobResourceClaim, PredictorFit, ResourceLease, SystemCursor, TaskEmbedding, Worker,
    };
    // A schema without foreign keys, so each row stands alone.
    let ddl: String = schema()?
        .lines()
        .filter(|line| !line.contains("FOREIGN KEY"))
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    let mut store = Store::open(&Location::Memory)?;
    store.write(|s| s.execute_batch(&ddl))?;
    let rows: Vec<Row> = vec![
        Commit {
            id: id(1),
            repository_id: id(2),
            sha: "abc".into(),
            tree_sha: None,
            parents: VectorSync::new_sync().push_back("p".to_owned()),
            author_name: "a".into(),
            author_email: "a@x".into(),
            authored_at: Some(at(1, 2)),
            committer_name: "c".into(),
            committer_email: "c@x".into(),
            committed_at: None,
            subject: "s".into(),
            message: "m".into(),
            trailers: VectorSync::new_sync().push_back(obj(json!({"Task-ID": "WG-1"}))),
            is_merge: false,
            patch_id: Some("pid".into()),
            insertions: 1,
            deletions: 2,
            files_changed: 3,
            reachable: true,
            unreachable_since: None,
            object_available: true,
            structure_analyzed: false,
            branches: VectorSync::new_sync().push_back("main".to_owned()),
            ingested_at: at(3, 0),
        }
        .into(),
        TaskEmbedding {
            id: id(3),
            task_id: id(4),
            provider: "p".into(),
            model: "m".into(),
            dimensions: 2,
            vector: [1u8, 2, 255, 0].into_iter().collect(),
            input_hash: "h".into(),
            created_at: at(4, 0),
        }
        .into(),
        Worker {
            id: "w-1".into(),
            hostname: "h".into(),
            pid: 7,
            status: "idle".into(),
            concurrency: 2,
            kinds: VectorSync::new_sync().push_back("k".to_owned()),
            current_jobs: VectorSync::new_sync(),
            jobs_completed: 0,
            jobs_failed: 0,
            started_at: at(5, 0),
            heartbeat_at: at(6, 0),
            stopped_at: None,
        }
        .into(),
        SystemCursor {
            name: "events".into(),
            value: 12,
            updated_at: at(7, 0),
        }
        .into(),
        ResourceLease {
            resource_key: "repo:x".into(),
            holder: "h".into(),
            purpose: "p".into(),
            acquired_at: at(8, 0),
            expires_at: Some(at(9, 0)),
        }
        .into(),
        JobResourceClaim {
            job_id: id(5),
            resource_key: "repo:x".into(),
        }
        .into(),
        PredictorFit {
            id: id(6),
            workspace_id: id(0),
            model: "ridge".into(),
            created_at: at(1, 0),
            data_cutoff: None,
            trial_ids: VectorSync::new_sync().push_back("t".to_owned()),
            feature_names: VectorSync::new_sync(),
            repositories: VectorSync::new_sync(),
            coef: VectorSync::new_sync()
                .push_back(json!(0.25))
                .push_back(json!(null)),
            penalty: VectorSync::new_sync().push_back(json!("l2")),
            n: 4,
            converged: true,
            metrics: obj(json!({"r2": 0.5})),
            samples: VectorSync::new_sync().push_back(json!([1, 2])),
        }
        .into(),
    ];
    let mut decision = Decision::new(());
    for row in &rows {
        decision = decision.write(Write::insert(row.clone()));
    }
    store.commit_decision(&decision)?;
    for row in &rows {
        assert_eq!(read_back(&mut store, &row.key())?, Some(row.clone()));
    }
    Ok(())
}
