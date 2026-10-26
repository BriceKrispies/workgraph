//! Verification-family checks (SH-10): config lookups, the newest run, filtered and paginated run
//! lists, check results in recorded order, the joined failure history, and acceptances.
//!
//! The database is built from `contracts/schema-0011.sql` (tables, the FTS virtual table and the
//! triggers), as `sh04_tasks.rs` builds it. Foreign keys are on, so the fixtures insert their
//! workspaces and tasks first.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::entity::{
    Task, TaskAcceptance, VerificationCheckResult, VerificationConfig, VerificationRun, Workspace,
};
use wg_model::{JsonObject, Row, Write, derive_id};
use wg_store::{FailureFilter, Location, Page, RunFilter, Store};
use wg_types::{Id, TaskKey, TaskPriority, TaskStatus, Timestamp, VerificationStatus, WgError};

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

fn put(store: &mut Store, row: impl Into<Row>) -> TestResult {
    store.write(|session| session.apply_write(&Write::insert(row)))?;
    Ok(())
}

/// The workspace with number `n`'s id; workspace 0 is the one most fixtures use.
fn workspace(n: u32, slug: &str) -> Workspace {
    Workspace {
        id: id(n),
        slug: slug.into(),
        name: slug.into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 10,
        settings: obj(json!({})),
        archived_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

/// A task in workspace `ws_n` with id `n` and key `WG-<number>`.
fn task(n: u32, ws_n: u32, number: i64) -> Task {
    Task {
        id: id(n),
        workspace_id: id(ws_n),
        number,
        key: ok(TaskKey::parse(&format!("WG-{number}"))),
        title: format!("task {number}"),
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

fn config(n: u32, ws_n: u32, scope_type: &str, scope: u32, name: &str) -> VerificationConfig {
    VerificationConfig {
        id: id(n),
        workspace_id: id(ws_n),
        scope_type: scope_type.into(),
        scope_id: id(scope),
        name: name.into(),
        checks: VectorSync::new_sync(),
        enabled: true,
        approved_by: "human:brice".into(),
        version: 1,
        created_at: at(1),
        updated_at: at(1),
    }
}

/// A run of task `task_n` in workspace `ws_n`, created at `second`.
fn run(
    n: u32,
    ws_n: u32,
    task_n: u32,
    attempt: Option<u32>,
    status: VerificationStatus,
    second: u8,
) -> VerificationRun {
    VerificationRun {
        id: id(n),
        workspace_id: id(ws_n),
        task_id: id(task_n),
        attempt_id: attempt.map(id),
        manifest_id: None,
        config_snapshot: VectorSync::new_sync(),
        status: status.as_str().into(),
        required_passed: status == VerificationStatus::Passed,
        triggered_by: "human:brice".into(),
        started_at: None,
        finished_at: None,
        created_at: at(second),
    }
}

fn check(
    n: u32,
    run_n: u32,
    name: &str,
    status: VerificationStatus,
    repository: Option<u32>,
    second: u8,
) -> VerificationCheckResult {
    VerificationCheckResult {
        id: id(n),
        run_id: id(run_n),
        name: name.into(),
        kind: "test".into(),
        required: true,
        repository_id: repository.map(id),
        status: status.as_str().into(),
        exit_code: Some(0),
        duration_ms: None,
        timed_out: false,
        log_artifact_id: None,
        output_excerpt: String::new(),
        failure_signatures: VectorSync::new_sync(),
        created_at: at(second),
    }
}

fn acceptance(n: u32, task_n: u32, decision: &str, second: u8) -> TaskAcceptance {
    TaskAcceptance {
        id: id(n),
        task_id: id(task_n),
        attempt_id: None,
        verification_run_id: None,
        decision: decision.into(),
        mode: "automated".into(),
        reason: String::new(),
        actor: "system".into(),
        manifest_id: None,
        created_at: at(second),
    }
}

/// Two workspaces: `default` (id 0) with tasks 1 and 2, and `other` (id 50) with task 3.
fn seeded() -> Result<Store, Box<dyn Error>> {
    let mut store = store()?;
    put(&mut store, workspace(0, "default"))?;
    put(&mut store, workspace(50, "other"))?;
    put(&mut store, task(1, 0, 1))?;
    put(&mut store, task(2, 0, 2))?;
    put(&mut store, task(3, 50, 3))?;
    Ok(store)
}

#[test]
fn configs_are_found_by_scope_and_listed_in_order() -> TestResult {
    let mut store = seeded()?;
    put(&mut store, config(10, 0, "workspace", 0, "lint"))?;
    put(&mut store, config(11, 0, "workspace", 0, "ci"))?;
    put(&mut store, config(12, 0, "repository", 70, "build"))?;
    put(&mut store, config(13, 50, "workspace", 50, "ci"))?;

    store.read(|session| {
        let found = ok(session.verification_config(id(0), "workspace", id(0), "ci"));
        assert_eq!(found.map(|c| c.id), Some(id(11)));
        assert!(ok(session.verification_config(id(0), "workspace", id(0), "missing")).is_none());
        assert!(ok(session.verification_config(id(0), "task", id(0), "ci")).is_none());
        // The same name under another scope is another config.
        let repo = ok(session.verification_config(id(0), "repository", id(70), "build"));
        assert_eq!(repo.map(|c| c.id), Some(id(12)));

        let names: Vec<String> = ok(session.verification_configs(id(0)))
            .into_iter()
            .map(|c| c.name)
            .collect();
        // Scope type, then name: the repository config sorts before the two workspace ones.
        assert_eq!(names, ["build", "ci", "lint"]);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn latest_run_is_the_newest_and_ties_break_on_id() -> TestResult {
    let mut store = seeded()?;
    put(
        &mut store,
        run(20, 0, 1, None, VerificationStatus::Failed, 1),
    )?;
    put(
        &mut store,
        run(21, 0, 1, None, VerificationStatus::Passed, 5),
    )?;
    put(
        &mut store,
        run(22, 0, 1, None, VerificationStatus::Failed, 3),
    )?;
    // Two runs in the same instant: the larger id text wins, as the query orders by `id DESC`.
    put(
        &mut store,
        run(23, 0, 1, None, VerificationStatus::Passed, 9),
    )?;
    put(
        &mut store,
        run(24, 0, 1, None, VerificationStatus::Error, 9),
    )?;
    put(
        &mut store,
        run(30, 0, 2, None, VerificationStatus::Passed, 59),
    )?;

    let tied_winner = [id(23), id(24)]
        .into_iter()
        .max_by_key(|id| id.to_string())
        .ok_or("no tied runs")?;

    store.read(|session| {
        let latest = ok(session.latest_verification_run(id(1)));
        assert_eq!(latest.map(|r| r.id), Some(tied_winner));
        assert!(ok(session.latest_verification_run(id(2))).is_some());
        assert!(ok(session.latest_verification_run(id(7_000))).is_none());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn run_lists_filter_and_page_newest_first() -> TestResult {
    let mut store = seeded()?;
    put(
        &mut store,
        run(40, 0, 1, Some(100), VerificationStatus::Failed, 1),
    )?;
    put(
        &mut store,
        run(41, 0, 1, Some(101), VerificationStatus::Passed, 2),
    )?;
    put(
        &mut store,
        run(42, 0, 2, Some(100), VerificationStatus::Failed, 3),
    )?;
    put(
        &mut store,
        run(43, 0, 1, Some(100), VerificationStatus::Passed, 4),
    )?;
    put(
        &mut store,
        run(44, 0, 2, None, VerificationStatus::Failed, 5),
    )?;
    put(
        &mut store,
        run(45, 50, 3, None, VerificationStatus::Failed, 6),
    )?;

    store.read(|session| {
        let all = ok(session.verification_runs(
            id(0),
            &RunFilter::default(),
            Page {
                limit: 50,
                offset: 0,
            },
        ));
        let order: Vec<Id> = all.iter().map(|r| r.id).collect();
        assert_eq!(order, [id(44), id(43), id(42), id(41), id(40)]);

        let by_task = ok(session.verification_runs(
            id(0),
            &RunFilter {
                task_id: Some(id(1)),
                ..RunFilter::default()
            },
            Page {
                limit: 50,
                offset: 0,
            },
        ));
        assert_eq!(
            by_task.iter().map(|r| r.id).collect::<Vec<_>>(),
            [id(43), id(41), id(40)]
        );

        let by_attempt = ok(session.verification_runs(
            id(0),
            &RunFilter {
                attempt_id: Some(id(100)),
                ..RunFilter::default()
            },
            Page {
                limit: 50,
                offset: 0,
            },
        ));
        assert_eq!(
            by_attempt.iter().map(|r| r.id).collect::<Vec<_>>(),
            [id(43), id(42), id(40)]
        );

        let combined = ok(session.verification_runs(
            id(0),
            &RunFilter {
                task_id: Some(id(1)),
                attempt_id: Some(id(100)),
                status: Some(VerificationStatus::Failed),
            },
            Page {
                limit: 50,
                offset: 0,
            },
        ));
        assert_eq!(combined.iter().map(|r| r.id).collect::<Vec<_>>(), [id(40)]);

        // Pages of two cover the list once, in the same order.
        let mut paged = Vec::new();
        for offset in [0, 2, 4] {
            let page = ok(session.verification_runs(
                id(0),
                &RunFilter::default(),
                Page { limit: 2, offset },
            ));
            paged.extend(page.into_iter().map(|r| r.id));
        }
        assert_eq!(paged, order);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn check_results_come_back_in_recorded_order() -> TestResult {
    let mut store = seeded()?;
    put(
        &mut store,
        run(50, 0, 1, None, VerificationStatus::Failed, 1),
    )?;
    put(
        &mut store,
        run(51, 0, 1, None, VerificationStatus::Passed, 2),
    )?;
    put(
        &mut store,
        check(60, 50, "lint", VerificationStatus::Passed, None, 3),
    )?;
    put(
        &mut store,
        check(61, 50, "unit", VerificationStatus::Failed, None, 1),
    )?;
    put(
        &mut store,
        check(62, 50, "e2e", VerificationStatus::Skipped, None, 2),
    )?;
    put(
        &mut store,
        check(63, 51, "unit", VerificationStatus::Passed, None, 4),
    )?;

    store.read(|session| {
        let names: Vec<String> = ok(session.verification_check_results(id(50)))
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["unit", "e2e", "lint"]);
        assert!(ok(session.verification_check_results(id(52))).is_empty());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn failure_history_joins_run_and_task_and_filters() -> TestResult {
    let mut store = seeded()?;
    // Task 1 (WG-1) has run 70 (attempt 7); task 2 (WG-2) has run 71 (no attempt).
    put(
        &mut store,
        run(70, 0, 1, Some(7), VerificationStatus::Failed, 1),
    )?;
    put(
        &mut store,
        run(71, 0, 2, None, VerificationStatus::Failed, 2),
    )?;
    put(
        &mut store,
        run(72, 50, 3, None, VerificationStatus::Failed, 3),
    )?;
    put(
        &mut store,
        check(80, 70, "unit", VerificationStatus::Failed, Some(90), 10),
    )?;
    put(
        &mut store,
        check(81, 70, "lint", VerificationStatus::Passed, Some(90), 11),
    )?;
    put(
        &mut store,
        check(82, 70, "e2e", VerificationStatus::Error, None, 12),
    )?;
    put(
        &mut store,
        check(83, 71, "unit", VerificationStatus::Failed, Some(91), 5),
    )?;
    put(
        &mut store,
        check(84, 71, "unit", VerificationStatus::Skipped, Some(91), 6),
    )?;
    put(
        &mut store,
        check(85, 72, "unit", VerificationStatus::Failed, None, 20),
    )?;

    let keys = |rows: &[wg_store::FailureRow]| -> Vec<(Id, String)> {
        rows.iter()
            .map(|r| (r.check.id, r.task_key.to_string()))
            .collect()
    };

    store.read(|session| {
        let everything = ok(session.verification_failures(
            id(0),
            &FailureFilter::default(),
            Page {
                limit: 200,
                offset: 0,
            },
        ));
        // Passed and skipped results are not failures; the other workspace is left out.
        assert_eq!(
            keys(&everything),
            [
                (id(82), "WG-1".to_owned()),
                (id(80), "WG-1".to_owned()),
                (id(83), "WG-2".to_owned()),
            ]
        );
        let first = &everything[0];
        assert_eq!(first.run_id, id(70));
        assert_eq!(first.attempt_id, Some(id(7)));
        assert_eq!(first.task_id, id(1));
        assert_eq!(first.check.status, "error");
        assert_eq!(everything[2].attempt_id, None);
        assert_eq!(everything[2].task_id, id(2));

        let unit = ok(session.verification_failures(
            id(0),
            &FailureFilter {
                check_name: Some("unit".into()),
                ..FailureFilter::default()
            },
            Page {
                limit: 200,
                offset: 0,
            },
        ));
        assert_eq!(
            keys(&unit),
            [(id(80), "WG-1".to_owned()), (id(83), "WG-2".to_owned())]
        );

        let in_repo = ok(session.verification_failures(
            id(0),
            &FailureFilter {
                repository_id: Some(id(91)),
                ..FailureFilter::default()
            },
            Page {
                limit: 200,
                offset: 0,
            },
        ));
        assert_eq!(keys(&in_repo), [(id(83), "WG-2".to_owned())]);

        let recent = ok(session.verification_failures(
            id(0),
            &FailureFilter {
                since: Some(at(10)),
                ..FailureFilter::default()
            },
            Page {
                limit: 200,
                offset: 0,
            },
        ));
        assert_eq!(
            keys(&recent),
            [(id(82), "WG-1".to_owned()), (id(80), "WG-1".to_owned())]
        );

        let second_page = ok(session.verification_failures(
            id(0),
            &FailureFilter::default(),
            Page {
                limit: 1,
                offset: 1,
            },
        ));
        assert_eq!(keys(&second_page), [(id(80), "WG-1".to_owned())]);

        let other = ok(session.verification_failures(
            id(50),
            &FailureFilter::default(),
            Page {
                limit: 200,
                offset: 0,
            },
        ));
        assert_eq!(keys(&other), [(id(85), "WG-3".to_owned())]);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn acceptances_of_a_task_are_oldest_first() -> TestResult {
    let mut store = seeded()?;
    put(&mut store, acceptance(90, 1, "rejected", 8))?;
    put(&mut store, acceptance(91, 1, "accepted", 4))?;
    put(&mut store, acceptance(92, 2, "accepted", 1))?;

    store.read(|session| {
        let decisions: Vec<String> = ok(session.task_acceptances(id(1)))
            .into_iter()
            .map(|a| a.decision)
            .collect();
        assert_eq!(decisions, ["accepted", "rejected"]);
        assert!(ok(session.task_acceptances(id(3))).is_empty());
        Ok(())
    })?;
    Ok(())
}
