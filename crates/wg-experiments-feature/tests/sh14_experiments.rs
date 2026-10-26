//! Experiment, predictor, external item and analytics source reads (SH-14) against real SQLite.
//!
//! Rows are written through the generic write path, then read back through the lookups and lists.
//! The database is built from `contracts/schema-0011.sql`, as the SH-04 to SH-07 checks do.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_experiments_feature::service::reads;
use wg_model::entity::{Experiment, ExperimentTrial, PredictorFit, Repository, Task, Workspace};
use wg_model::{JsonObject, Row, Write, derive_id};
use wg_store::{Location, Store};
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

fn experiment(n: u32, slug: &str, status: &str, created: u8) -> Experiment {
    Experiment {
        id: id(n),
        workspace_id: id(0),
        slug: slug.into(),
        title: format!("experiment {slug}"),
        question: None,
        status: status.into(),
        protocol: obj(json!({"arms": [{"name": "a", "model": "m1"}]})),
        frozen_at: None,
        created_at: at(created),
        updated_at: at(created),
    }
}

fn trial(n: u32, experiment: u32, task: u32, seq: i64) -> ExperimentTrial {
    ExperimentTrial {
        id: id(n),
        experiment_id: id(experiment),
        task_id: id(task),
        attempt_id: None,
        subagent_run_id: None,
        arm: "a".into(),
        requested_model: "m1".into(),
        actual_model: None,
        phase: "measure".into(),
        seq,
        band: None,
        features: obj(json!({})),
        base_ref: None,
        worktree_path: None,
        status: "planned".into(),
        outcome: None,
        outcome_detail: obj(json!({})),
        failure_cause: None,
        failure_cause_source: None,
        notes: None,
        created_at: at(seq as u8),
        started_at: None,
        graded_at: None,
    }
}

fn fit(n: u32, model: &str, created: u8) -> PredictorFit {
    PredictorFit {
        id: id(n),
        workspace_id: id(0),
        model: model.into(),
        created_at: at(created),
        data_cutoff: None,
        trial_ids: VectorSync::new_sync(),
        feature_names: VectorSync::new_sync(),
        repositories: VectorSync::new_sync(),
        coef: VectorSync::new_sync(),
        penalty: VectorSync::new_sync(),
        n: 4,
        converged: true,
        metrics: obj(json!({})),
        samples: VectorSync::new_sync(),
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

/// Two experiments in workspace 0: "alpha" (created first, running) with three trials added in
/// the order seq 2, 1, 3 (task 1 is WG-1, task 2 is WG-2), and "beta" (closed) with none.
fn experiment_fixture(store: &mut Store) -> TestResult {
    insert_all(
        store,
        vec![
            Row::Workspace(workspace()),
            Row::Repository(repository(20, "alpha")),
            Row::Task(task(1, 1)),
            Row::Task(task(2, 2)),
            Row::Task(task(3, 3)),
            Row::Experiment(experiment(50, "beta", "closed", 7)),
            Row::Experiment(experiment(51, "alpha", "running", 6)),
            Row::ExperimentTrial(trial(60, 51, 2, 2)),
            Row::ExperimentTrial(trial(61, 51, 1, 1)),
            Row::ExperimentTrial(trial(62, 51, 3, 3)),
        ],
    )
}

#[test]
fn experiment_lookups_by_id_and_slug() -> TestResult {
    let mut store = store()?;
    experiment_fixture(&mut store)?;

    store.read(|session| {
        let by_id = reads::experiment_by_id(session, id(51))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "experiment 51 missing"))?;
        assert_eq!(by_id.slug, "alpha");
        assert_eq!(by_id.status, "running");
        assert!(reads::experiment_by_id(session, id(99))?.is_none());

        let by_slug = reads::experiment_by_slug(session, id(0), "beta")?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "experiment beta missing"))?;
        assert_eq!(by_slug.id, id(50));
        assert!(reads::experiment_by_slug(session, id(0), "gamma")?.is_none());
        assert!(reads::experiment_by_slug(session, id(9), "beta")?.is_none());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn experiment_list_is_oldest_first_and_filters_by_status() -> TestResult {
    let mut store = store()?;
    experiment_fixture(&mut store)?;

    store.read(|session| {
        // Python orders by created_at: "alpha" (6) before "beta" (7).
        let all: Vec<Id> = reads::experiments_in_workspace(session, id(0), None)?
            .iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(all, vec![id(51), id(50)]);

        let running: Vec<Id> = reads::experiments_in_workspace(session, id(0), Some("running"))?
            .iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(running, vec![id(51)]);
        assert!(reads::experiments_in_workspace(session, id(0), Some("draft"))?.is_empty());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn trials_come_in_seq_order_with_task_keys_and_counts() -> TestResult {
    let mut store = store()?;
    experiment_fixture(&mut store)?;

    store.read(|session| {
        let seqs: Vec<i64> = reads::trials_of_experiment(session, id(51))?
            .iter()
            .map(|t| t.seq)
            .collect();
        assert_eq!(seqs, vec![1, 2, 3]);

        let joined = reads::trials_with_task_keys(session, id(51))?;
        let keys: Vec<(i64, String)> = joined
            .iter()
            .map(|row| (row.trial.seq, row.task_key.clone()))
            .collect();
        assert_eq!(
            keys,
            vec![
                (1, "WG-1".to_owned()),
                (2, "WG-2".to_owned()),
                (3, "WG-3".to_owned()),
            ]
        );
        assert_eq!(joined[1].trial.id, id(60));

        assert_eq!(reads::trial_count(session, id(51))?, 3);
        assert_eq!(reads::max_trial_seq(session, id(51))?, Some(3));
        // An experiment with no trials counts zero and has no highest sequence number.
        assert_eq!(reads::trial_count(session, id(50))?, 0);
        assert_eq!(reads::max_trial_seq(session, id(50))?, None);

        let by_id = reads::trial_by_id(session, id(61))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "trial 61 missing"))?;
        assert_eq!(by_id.seq, 1);
        assert!(reads::trial_by_id(session, id(99))?.is_none());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn latest_predictor_fit_is_the_newest_of_the_model() -> TestResult {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            Row::Workspace(workspace()),
            Row::PredictorFit(fit(70, "logit", 3)),
            Row::PredictorFit(fit(71, "logit", 5)),
            Row::PredictorFit(fit(72, "other", 9)),
        ],
    )?;

    store.read(|session| {
        let latest = reads::latest_predictor_fit(session, id(0), "logit")?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "no fit"))?;
        assert_eq!(latest.id, id(71));
        assert!(reads::latest_predictor_fit(session, id(0), "missing")?.is_none());

        let by_id = reads::predictor_fit_by_id(session, id(70))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "fit 70 missing"))?;
        assert_eq!(by_id.model, "logit");
        assert!(reads::predictor_fit_by_id(session, id(99))?.is_none());
        Ok(())
    })?;
    Ok(())
}
