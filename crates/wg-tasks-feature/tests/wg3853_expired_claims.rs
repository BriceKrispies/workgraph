//! WG-3853: the expired-claim maintenance step (`release_expired_claims`) against a real database.
//! A claim whose lease ran out goes back to ready with the lease reason; a live claim is left alone.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::Map;
use wg_model::entity::{Task, Workspace};
use wg_model::{Decision, Write, derive_id};
use wg_runtime::{App, system_context};
use wg_tasks::claims::REASON_LEASE_EXPIRED;
use wg_tasks_feature::service::release_expired_claims;
use wg_types::{Id, TaskKey, TaskPriority, TaskStatus, Timestamp};

fn at(iso: &str) -> Timestamp {
    Timestamp::parse_iso(iso).expect("a timestamp")
}

fn id(n: u32) -> Id {
    derive_id(&[9; 32], n)
}

fn task(n: u32, claimed_by: &str, expires: &str) -> Task {
    Task {
        id: id(10 + n),
        workspace_id: id(0),
        number: i64::from(n),
        key: TaskKey::parse(&format!("WG-{n}")).expect("a key"),
        title: format!("task {n}"),
        description: String::new(),
        description_hash: "h".into(),
        acceptance_criteria: Default::default(),
        status: TaskStatus::Claimed,
        priority: TaskPriority::Medium,
        category: None,
        parent_id: None,
        assignee: None,
        assigned_agent: None,
        claimed_by: Some(claimed_by.into()),
        claim_expires_at: Some(at(expires)),
        auto_execute: false,
        estimated_effort: None,
        metadata: Map::new(),
        due_at: None,
        scheduled_at: None,
        created_by: "human:brice".into(),
        version: 1,
        created_at: at("2026-10-26T09:00:00Z"),
        updated_at: at("2026-10-26T09:00:00Z"),
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    }
}

fn home(name: &str) -> App {
    let dir = std::env::temp_dir().join(format!("wg3853-claims-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = App::open(Some(&dir)).expect("the app opens");
    let decision = Decision::new(())
        .write(Write::insert(Workspace {
            id: id(0),
            slug: "default".into(),
            name: "Default".into(),
            description: String::new(),
            key_prefix: "WG".into(),
            next_task_number: 10,
            settings: Map::new(),
            archived_at: None,
            created_at: at("2026-10-26T09:00:00Z"),
            updated_at: at("2026-10-26T09:00:00Z"),
        }))
        // WG-1's lease ended long ago; WG-2's runs until 2099.
        .write(Write::insert(task(1, "agent:old", "2026-10-26T10:00:00Z")))
        .write(Write::insert(task(2, "agent:live", "2099-01-01T00:00:00Z")));
    app.runtime
        .store
        .commit_decision(&decision)
        .expect("fixtures written");
    app
}

fn status_of(app: &mut App, n: u32) -> (TaskStatus, Option<String>) {
    let world = app
        .runtime
        .store
        .read(|s| s.task_world(None, &[]))
        .expect("the tasks read");
    let row = world.tasks.task(id(10 + n)).expect("the task exists");
    (row.status, row.claimed_by.clone())
}

#[test]
fn an_expired_claim_goes_back_to_ready_and_a_live_one_stays() {
    let mut app = home("sweep");
    let context = system_context("maintenance").expect("the system context");
    let released = release_expired_claims(&mut app, &context).expect("the sweep runs");
    assert_eq!(released, vec!["WG-1".to_owned()]);
    assert_eq!(status_of(&mut app, 1), (TaskStatus::Ready, None));
    assert_eq!(
        status_of(&mut app, 2),
        (TaskStatus::Claimed, Some("agent:live".to_owned()))
    );
    assert_eq!(REASON_LEASE_EXPIRED, "claim lease expired");
}

#[test]
fn a_second_sweep_releases_nothing() {
    let mut app = home("again");
    let context = system_context("maintenance").expect("the system context");
    release_expired_claims(&mut app, &context).expect("the sweep runs");
    let again = release_expired_claims(&mut app, &context).expect("the sweep runs");
    assert!(again.is_empty());
}
