//! Structured-decision checks (`task_decisions`): a decision written with `apply_decision` reads
//! back unchanged, its event is appended in the same transaction, and the list queries order and
//! filter the rows as Python's `DecisionService` does.
//!
//! The database is built from `contracts/schema-0011.sql` (tables, the FTS virtual table and the
//! triggers), as `sh04_tasks.rs` builds it.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::entity::{DecisionOption, Task, TaskDecision, Workspace};
use wg_model::event::TaskDecisionChanged;
use wg_model::{Decision, EventDraft, EventPayload, JsonObject, Stamp, Write, derive_id};
use wg_store::{EventQuery, Location, Store};
use wg_storecodec::decision_sql::DecisionFilter;
use wg_types::{Actor, Id, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError};

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
    ok(Timestamp::from_components(2026, 10, 25, 12, 0, second, 0))
}

fn obj(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

fn stamp() -> Stamp {
    Stamp {
        at: at(59),
        entropy: entropy(9),
        actor: ok(Actor::parse("human:brice")),
        permissions: Default::default(),
        correlation: id(900).into(),
        causation: None,
        attempt: None,
        automation_depth: 0,
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

fn task(n: u32, priority: TaskPriority, archived: bool) -> Task {
    Task {
        id: id(n),
        workspace_id: id(0),
        number: i64::from(n),
        key: ok(TaskKey::parse(&format!("WG-{n}"))),
        title: format!("task {n}"),
        description: String::new(),
        description_hash: "h".into(),
        acceptance_criteria: VectorSync::new_sync(),
        status: TaskStatus::Ready,
        priority,
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
        archived_at: archived.then(|| at(2)),
        current_classification_id: None,
    }
}

fn options() -> Vec<DecisionOption> {
    vec![
        DecisionOption {
            id: "a".into(),
            label: "SQLite".into(),
            detail: "Local file".into(),
            recommended: true,
        },
        DecisionOption {
            id: "b".into(),
            label: "Postgres".into(),
            detail: String::new(),
            recommended: false,
        },
    ]
}

/// A decision row on `task_n`, created at `second`.
fn decision(n: u32, task_n: u32, second: u8, status: &str) -> TaskDecision {
    TaskDecision {
        id: id(n),
        workspace_id: id(0),
        task_id: id(task_n),
        question: format!("question {n}?"),
        options: options(),
        status: status.into(),
        chosen_option_id: None,
        other_text: None,
        note: None,
        decided_by: None,
        decided_at: None,
        created_by: "human:brice".into(),
        created_at: at(second),
        updated_at: at(second),
        version: 1,
    }
}

#[test]
fn a_decision_written_with_apply_decision_reads_back_and_the_queries_find_it() -> TestResult {
    let mut store = store()?;
    let open_high = task(1, TaskPriority::High, false);
    let open_low = task(2, TaskPriority::Low, false);
    let archived = task(3, TaskPriority::Critical, true);
    store.write(|s| {
        s.apply_write(&Write::insert(workspace()))?;
        s.apply_write(&Write::insert(open_high.clone()))?;
        s.apply_write(&Write::insert(open_low.clone()))?;
        s.apply_write(&Write::insert(archived.clone()))
    })?;

    // d1: an open decision on the high-priority task. Written as Python's `request` does it: the
    // row and its `task.decision_requested` event in one decision.
    let d1 = decision(10, 1, 10, "open");
    let event = EventDraft::new(
        &stamp(),
        id(500),
        Some(id(0)),
        open_high.id,
        EventPayload::TaskDecisionRequested(TaskDecisionChanged {
            key: open_high.key.clone(),
            decision_id: d1.id,
            question: d1.question.clone(),
            version: d1.version,
        }),
    );
    let applied = store.write(|s| {
        s.apply_decision(
            &Decision::new(())
                .write(Write::insert(d1.clone()))
                .event(event),
        )
    })?;
    assert_eq!(applied.event_seqs.len(), 1);

    // The rest of the fixture: an open decision on the low-priority task, older than d1; an open
    // decision on an archived task; a decided one on the high-priority task.
    let d4 = decision(13, 2, 5, "open");
    let d2 = decision(11, 3, 11, "open");
    let mut d3 = decision(12, 1, 12, "decided");
    d3.chosen_option_id = Some("b".into());
    d3.decided_by = Some("human:brice".into());
    d3.decided_at = Some(at(30));
    d3.version = 2;
    store.write(|s| {
        for row in [&d4, &d2, &d3] {
            s.apply_write(&Write::insert(row.clone()))?;
        }
        Ok(())
    })?;

    // Read back: the row as written, the event as appended.
    let read = store.read(|s| s.decision_by_id(id(0), d1.id))?;
    assert_eq!(read, Some(d1.clone()));
    assert_eq!(store.read(|s| s.decision_by_id(id(99), d1.id))?, None);

    let events = store.read(|s| {
        s.query_events(&EventQuery {
            event_types: vec!["task.decision_requested".into()],
            ..EventQuery::default()
        })
    })?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].entity_id, open_high.id);
    assert_eq!(events[0].data["decision_id"], json!(d1.id.to_string()));
    assert_eq!(events[0].data["version"], json!(1));

    // Per task: oldest first. Open count: one open decision on the high-priority task.
    let for_task = store.read(|s| s.decisions_for_task(open_high.id))?;
    assert_eq!(for_task, vec![d1.clone(), d3.clone()]);
    assert_eq!(store.read(|s| s.open_decision_count(open_high.id))?, 1);
    assert_eq!(store.read(|s| s.open_decision_count(archived.id))?, 1);

    // Open: by task priority (high before low), archived tasks left out.
    let open = store.read(|s| s.decisions_by_status(id(0), DecisionFilter::Open, 200))?;
    assert_eq!(open, vec![d1.clone(), d4.clone()]);

    // Decided: the answered decision only.
    let decided = store.read(|s| s.decisions_by_status(id(0), DecisionFilter::Decided, 200))?;
    assert_eq!(decided, vec![d3.clone()]);

    // Withdrawn: none. All: newest created first, archived task included, limit respected.
    let withdrawn = store.read(|s| s.decisions_by_status(id(0), DecisionFilter::Withdrawn, 200))?;
    assert!(withdrawn.is_empty());
    let all = store.read(|s| s.decisions_by_status(id(0), DecisionFilter::All, 200))?;
    assert_eq!(all, vec![d3.clone(), d2.clone(), d1.clone(), d4.clone()]);
    let newest_two = store.read(|s| s.decisions_by_status(id(0), DecisionFilter::All, 2))?;
    assert_eq!(newest_two, vec![d3, d2]);
    Ok(())
}

#[test]
fn briefs_carry_the_task_and_its_sorted_tags_in_the_same_order_as_the_bare_lists() -> TestResult {
    use wg_model::entity::TaskTag;
    use wg_storecodec::decision_sql::{DecisionFilter as F, DecisionTask};
    use wg_types::TaskKey as Key;

    let mut store = store()?;
    let mut tagged = task(1, TaskPriority::High, false);
    tagged.title = "tagged".into();
    tagged.description = "why".into();
    let plain = task(2, TaskPriority::Low, false);
    store.write(|s| {
        s.apply_write(&Write::insert(workspace()))?;
        s.apply_write(&Write::insert(tagged.clone()))?;
        s.apply_write(&Write::insert(plain.clone()))?;
        for tag in ["zeta", "alpha"] {
            s.apply_write(&Write::insert(TaskTag {
                task_id: tagged.id,
                tag: tag.into(),
            }))?;
        }
        Ok(())
    })?;
    let older_low = decision(30, 2, 4, "open");
    let newer_high = decision(31, 1, 8, "open");
    store.write(|s| {
        s.apply_write(&Write::insert(older_low.clone()))?;
        s.apply_write(&Write::insert(newer_high.clone()))
    })?;

    let bare = store.read(|s| s.decisions_by_status(id(0), F::Open, 200))?;
    let briefs = store.read(|s| s.decisions_by_status_with_task(id(0), F::Open, 200))?;
    let brief_decisions: Vec<TaskDecision> = briefs.iter().map(|b| b.decision.clone()).collect();
    assert_eq!(brief_decisions, bare);
    assert_eq!(bare, vec![newer_high.clone(), older_low.clone()]);

    let first = &briefs[0].task;
    assert_eq!(
        first,
        &DecisionTask {
            key: ok(Key::parse("WG-1")),
            title: "tagged".into(),
            status: TaskStatus::Ready,
            priority: TaskPriority::High,
            description: "why".into(),
            tags: vec!["alpha".into(), "zeta".into()],
        }
    );
    assert!(briefs[1].task.tags.is_empty());

    let one = store.read(|s| s.decision_by_id_with_task(id(0), newer_high.id))?;
    assert_eq!(one.map(|b| b.task), Some(first.clone()));
    assert_eq!(
        store.read(|s| s.decision_by_id_with_task(id(0), id(99)))?,
        None
    );
    Ok(())
}
