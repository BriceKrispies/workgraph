#![allow(clippy::expect_used)]
//! CF-06 and O13, the event log's read path: the vocabulary of `contracts/events.json`, the upcast of
//! `task.created` v1 to v2 on read, and the `events verify` replay of the task events.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use wg_events::{
    Mismatch, ProjectionError, TaskState, current_version, project_tasks, read, upcast,
    verify_task_projection,
};
use wg_model::{JsonObject, StoredEvent};
use wg_types::{Correlation, Id, Timestamp};

fn contract() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/events.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn object(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        _ => JsonObject::new(),
    }
}

fn task_id(n: u128) -> Id {
    Id::from_u128(n)
}

/// A stored task event at `seq`, under the given schema version.
fn event(seq: i64, event_type: &str, entity: Id, data: Value, schema_version: i64) -> StoredEvent {
    StoredEvent {
        seq,
        id: Id::from_u128(10_000 + seq as u128),
        workspace_id: Some(Id::from_u128(7)),
        event_type: event_type.to_owned(),
        entity_type: "task".to_owned(),
        entity_id: entity,
        actor: "human:Brice".to_owned(),
        actor_kind: "human".to_owned(),
        occurred_at: Timestamp::parse_iso("2026-10-26T09:00:00Z").expect("a timestamp"),
        correlation_id: Correlation::new("corr"),
        causation_id: None,
        attempt_id: None,
        schema_version,
        data: object(data),
    }
}

/// The task events of a small workspace: two tasks, a dependency, an update, an archive, a status
/// change, and an event of a type the projection does not read.
fn task_log() -> (Vec<StoredEvent>, Id, Id) {
    let a = task_id(1);
    let b = task_id(2);
    let a_text = a.to_string();
    let events = vec![
        event(
            1,
            "task.created",
            a,
            json!({"key": "WG-1", "title": "A", "description": "", "status": "backlog",
                   "priority": "medium", "tags": ["one"], "project_ids": [], "repository_ids": [],
                   "parent_id": null, "depends_on": [], "assignee": null, "auto_execute": false,
                   "acceptance_criteria": []}),
            2,
        ),
        // v1 payload: tags were one comma-separated string, no acceptance criteria.
        event(
            2,
            "task.created",
            b,
            json!({"key": "WG-2", "title": "B", "description": "", "status": "backlog",
                   "priority": "medium", "tags": "x, y", "parent_id": a_text}),
            1,
        ),
        event(
            3,
            "task.dependency_added",
            b,
            json!({"depends_on": a_text}),
            1,
        ),
        event(
            4,
            "task.updated",
            a,
            json!({"changes": {"title": ["A", "A2"], "tags": [["one"], ["two"]], "priority": ["medium", "low"]}}),
            1,
        ),
        event(5, "task.archived", a, json!({"key": "WG-1"}), 1),
        event(6, "task.status_changed", b, json!({"to": "running"}), 1),
        event(7, "task.relation_added", a, json!({"target": "WG-2"}), 1),
    ];
    (events, a, b)
}

fn table_a(id: Id) -> TaskState {
    TaskState {
        id,
        key: "WG-1".to_owned(),
        title: "A2".to_owned(),
        description: String::new(),
        status: "backlog".to_owned(),
        priority: "low".to_owned(),
        tags: vec!["two".to_owned()],
        project_ids: Vec::new(),
        repository_ids: Vec::new(),
        parent_id: None,
        depends_on: Vec::new(),
        archived: true,
    }
}

fn table_b(id: Id, parent: Id) -> TaskState {
    TaskState {
        id,
        key: "WG-2".to_owned(),
        title: "B".to_owned(),
        description: String::new(),
        status: "running".to_owned(),
        priority: "medium".to_owned(),
        tags: vec!["x".to_owned(), "y".to_owned()],
        project_ids: Vec::new(),
        repository_ids: Vec::new(),
        parent_id: Some(parent.to_string()),
        depends_on: vec![parent.to_string()],
        archived: false,
    }
}

#[test]
fn every_event_type_the_projection_reads_is_in_the_vocabulary() {
    let vocabulary = contract();
    let events = &vocabulary["events"];
    for name in [
        "task.created",
        "task.updated",
        "task.status_changed",
        "task.dependency_added",
        "task.dependency_removed",
        "task.repository_linked",
        "task.repository_unlinked",
        "task.archived",
        "task.unarchived",
    ] {
        let entity_types = events[name]["entity_types"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            entity_types.contains(&json!("task")),
            "{name} is a task event in CF-06"
        );
    }
}

#[test]
fn task_created_schema_version_is_the_vocabulary_version() {
    assert_eq!(current_version("task.created"), 2);
    assert_eq!(current_version("task.updated"), 1);
}

// covers: O13
#[test]
fn upcast_v1_task_created_keeps_to_the_vocabulary_keys() {
    let vocabulary = contract();
    let allowed: Vec<String> = vocabulary["events"]["task.created"]["data_keys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|key| key.as_str().map(str::to_owned))
        .collect();
    let v1 = object(json!({"key": "WG-9", "tags": "a,b", "title": "t"}));
    let (version, data) = upcast("task.created", 1, v1);
    assert_eq!(version, 2);
    assert_eq!(data["tags"], json!(["a", "b"]));
    assert_eq!(data["acceptance_criteria"], json!([]));
    for key in data.keys() {
        assert!(allowed.contains(key), "{key} is not in CF-06 task.created");
    }
}

// covers: O13
#[test]
fn read_upcasts_the_stored_record() {
    let stored = event(
        1,
        "task.created",
        task_id(3),
        json!({"key": "WG-3", "tags": "p"}),
        1,
    );
    let record = read(stored);
    assert_eq!(record.schema_version, 2);
    assert_eq!(record.data["tags"], json!(["p"]));
}

// covers: C067
#[test]
fn a_faithful_log_verifies_consistent() {
    let (events, a, b) = task_log();
    let tables = [table_a(a), table_b(b, a)];
    let report = verify_task_projection(events, &tables).expect("a replayable log");
    assert!(report.consistent(), "{:?}", report.mismatches);
    assert_eq!(report.tasks, 2);
    assert_eq!(report.events, 7);
    assert_eq!(report.to_json()["consistent"], json!(true));
}

// covers: C067
#[test]
fn a_stale_field_is_reported_with_both_values() {
    let (events, a, b) = task_log();
    let stale = TaskState {
        title: "A-old".to_owned(),
        ..table_a(a)
    };
    let report = verify_task_projection(events, &[stale, table_b(b, a)]).expect("a replayable log");
    assert!(!report.consistent());
    let json = report.to_json();
    assert_eq!(json["mismatches"]["WG-1"]["title"]["table"], json!("A-old"));
    assert_eq!(json["mismatches"]["WG-1"]["title"]["events"], json!("A2"));
    assert_eq!(report.mismatches.len(), 1);
}

#[test]
fn a_task_without_task_created_is_no_created() {
    let (events, a, b) = task_log();
    let orphan = TaskState {
        id: task_id(99),
        key: "WG-99".to_owned(),
        ..table_a(a)
    };
    let report = verify_task_projection(events, &[table_a(a), table_b(b, a), orphan])
        .expect("a replayable log");
    assert_eq!(
        report.mismatches,
        vec![("WG-99".to_owned(), Mismatch::NoCreated)]
    );
    assert_eq!(
        report.to_json()["mismatches"]["WG-99"]["error"],
        json!("no task.created event")
    );
}

#[test]
fn the_replay_applies_updates_dependencies_and_archives() {
    let (events, a, b) = task_log();
    let replayed = project_tasks(events.into_iter().map(read).collect()).expect("a replayable log");
    let task_a = replayed.get(&a).expect("task A");
    assert_eq!(task_a.title, json!("A2"));
    assert_eq!(task_a.priority, json!("low"));
    assert_eq!(task_a.tags, vec!["two".to_owned()]);
    assert!(task_a.archived);
    let task_b = replayed.get(&b).expect("task B");
    assert_eq!(task_b.status, json!("running"));
    assert_eq!(task_b.depends_on, vec![a.to_string()]);
    // A dependency added twice is recorded once.
    let twice = vec![
        event(1, "task.created", b, json!({"key": "WG-2"}), 2),
        event(2, "task.dependency_added", b, json!({"depends_on": "x"}), 1),
        event(3, "task.dependency_added", b, json!({"depends_on": "x"}), 1),
    ];
    let replayed = project_tasks(twice).expect("a replayable log");
    assert_eq!(
        replayed.get(&b).map(|task| task.depends_on.clone()),
        Some(vec!["x".to_owned()])
    );
}

#[test]
fn a_payload_missing_a_read_field_is_an_error() {
    let a = task_id(5);
    let log = vec![
        event(1, "task.created", a, json!({"key": "WG-5"}), 2),
        event(2, "task.status_changed", a, json!({}), 1),
    ];
    let error = project_tasks(log).expect_err("no `to`");
    assert_eq!(
        error,
        ProjectionError::MissingField {
            seq: 2,
            event_type: "task.status_changed".to_owned(),
            field: "to".to_owned(),
        }
    );
}
