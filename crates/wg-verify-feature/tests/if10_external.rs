//! External acceptance (the port of `VerificationService.record_external_acceptance`): a tracker
//! sync whose issue is done records an acceptance with no verification run, the `task.accepted`
//! event and the change-surface job it queues. The event id is the decision's reply.
//!
//! The database is built from `contracts/schema-0011.sql`, as `sh10_verify.rs` builds it.

use std::error::Error;
use std::path::PathBuf;

use rpds::{RedBlackTreeSetSync, VectorSync};
use serde_json::{Value, json};
use wg_jobs::queue::JobFilter;
use wg_model::entity::{Task, Workspace};
use wg_model::stamp::Stamp;
use wg_model::{JsonObject, Row, Write, derive_id};
use wg_store::{EventQuery, Location, Store};
use wg_types::{
    Actor, ActorKind, Correlation, Id, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError,
};
use wg_verify_feature::service::external::record_external_acceptance;

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
    derive_id(&entropy(3), n)
}

fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 26, 12, 0, second, 0))
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
        id: id(1),
        slug: "ws".into(),
        name: "ws".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 10,
        settings: obj(json!({})),
        archived_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

fn task() -> Task {
    Task {
        id: id(2),
        workspace_id: id(1),
        number: 7,
        key: ok(TaskKey::parse("WG-7")),
        title: "tracked task".into(),
        description: String::new(),
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

fn stamp() -> Stamp {
    Stamp {
        at: at(30),
        entropy: entropy(9),
        actor: ok(Actor::new(ActorKind::System, "tracker-sync")),
        permissions: RedBlackTreeSetSync::new_sync(),
        correlation: Correlation::new("corr-external"),
        causation: None,
        attempt: None,
        automation_depth: 0,
    }
}

#[test]
fn external_acceptance_records_row_event_and_surface_job() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace())?;
    let task = task();
    put(&mut store, task.clone())?;
    let stamp = stamp();

    // The caller's transaction: the decision is built from the session and applied to it.
    let event_id = store.write(|session| {
        let decision = record_external_acceptance(session, &stamp, &task, "jira")?;
        session.apply_decision(&decision)?;
        Ok(decision.reply)
    })?;

    let acceptances = store.read(|session| session.task_acceptances(task.id))?;
    assert_eq!(acceptances.len(), 1, "one acceptance row");
    let row = &acceptances[0];
    assert_eq!(row.decision, "accepted");
    assert_eq!(row.mode, "external");
    assert_eq!(row.reason, "external:jira");
    assert_eq!(row.verification_run_id, None);
    assert_eq!(row.attempt_id, None, "the task has no succeeded attempt");
    assert_eq!(row.manifest_id, None);
    assert_eq!(row.actor, stamp.actor.to_string());

    let events = store.read(|session| {
        session.query_events(&EventQuery {
            entity_ids: vec![task.id],
            event_types: vec!["task.accepted".to_owned()],
            ..EventQuery::default()
        })
    })?;
    assert_eq!(events.len(), 1, "one task.accepted event");
    assert_eq!(events[0].id, event_id, "the reply is the event id");
    assert_eq!(events[0].data.get("reason"), Some(&json!("external:jira")));
    assert_eq!(events[0].data.get("mode"), Some(&json!("external")));

    let (jobs, _) = store.read(|session| {
        session.list_jobs(&JobFilter {
            statuses: &[],
            kind: Some("compute_change_surface"),
            workspace_id: None,
            task_id: Some(task.id),
            offset: 0,
            limit: None,
        })
    })?;
    assert_eq!(jobs.len(), 1, "the change surface is queued once");
    assert_eq!(jobs[0].causation_id, Some(event_id));
    Ok(())
}
