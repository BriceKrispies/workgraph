//! The trial commands' decisions (`trial add`, `trial classify`) against the Python service's
//! messages and checks, over in-memory fixtures. The shell's reads are covered by the store tests.

use rpds::RedBlackTreeSetSync;
use rpds::VectorSync;
use serde_json::{Value, json};
use wg_clispec::args::{get, s};
use wg_experiments_feature::spec::trial::{self, AddInput};
use wg_model::entity::{Experiment, ExperimentTrial, Task};
use wg_model::stamp::Stamp;
use wg_model::{JsonObject, RowKey, derive_id};
use wg_types::{
    Actor, ErrorCode, Id, Permission, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError,
};

const ACTOR: &str = "human:tester";

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn id(n: u32) -> Id {
    derive_id(&entropy(14), n)
}

fn at(second: u8) -> Timestamp {
    match Timestamp::from_components(2026, 10, 25, 12, 0, second, 0) {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn obj(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

fn stamp() -> Stamp {
    Stamp {
        at: at(1),
        entropy: entropy(7),
        actor: Actor::parse(ACTOR).unwrap_or_else(|e| panic!("{e}")),
        permissions: RedBlackTreeSetSync::new_sync().insert(Permission::ExperimentWrite),
        correlation: id(9).into(),
        causation: None,
        attempt: None,
        automation_depth: 0,
    }
}

fn experiment(status: &str, protocol: Value) -> Experiment {
    Experiment {
        id: id(1),
        workspace_id: id(0),
        slug: "exp".into(),
        title: "experiment exp".into(),
        question: None,
        status: status.into(),
        protocol: obj(protocol),
        frozen_at: None,
        created_at: at(1),
        updated_at: at(1),
    }
}

fn running(bands: Value) -> Experiment {
    experiment(
        "running",
        json!({"arms": [{"name": "a", "model": "m1"}], "bands": bands}),
    )
}

fn task() -> Task {
    Task {
        id: id(2),
        workspace_id: id(0),
        number: 7,
        key: TaskKey::parse("WG-7").unwrap_or_else(|e| panic!("{e}")),
        title: "task 7".into(),
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

fn trial(status: &str, outcome: Option<&str>) -> ExperimentTrial {
    ExperimentTrial {
        id: id(3),
        experiment_id: id(1),
        task_id: id(2),
        attempt_id: None,
        subagent_run_id: None,
        arm: "a".into(),
        requested_model: "m1".into(),
        actual_model: None,
        phase: "staircase".into(),
        seq: 1,
        band: Some(2),
        features: obj(json!({})),
        base_ref: None,
        worktree_path: None,
        status: status.into(),
        outcome: outcome.map(str::to_owned),
        outcome_detail: obj(json!({})),
        failure_cause: None,
        failure_cause_source: None,
        notes: Some("kept".into()),
        created_at: at(1),
        started_at: None,
        graded_at: None,
    }
}

fn add_input<'a>(arm: &'a str, phase: &'a str, band: Option<i64>) -> AddInput<'a> {
    AddInput { arm, phase, band }
}

fn error_of<T: std::fmt::Debug>(result: Result<T, WgError>) -> WgError {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(err) => err,
    }
}

// covers: C079
#[test]
fn add_refuses_an_experiment_that_is_not_running() {
    let draft = experiment("draft", json!({"arms": [], "bands": {"min": 1, "max": 3}}));
    let err = error_of(trial::add(
        &draft,
        &task(),
        1,
        add_input("a", "staircase", None),
        &stamp(),
    ));
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(
        err.message,
        "experiment exp is draft; trials change only while it is running"
    );
}

// covers: C079
#[test]
fn add_names_the_arms_of_the_protocol_when_the_arm_is_unknown() {
    let experiment = running(json!({"min": 1, "max": 3}));
    let err = error_of(trial::add(
        &experiment,
        &task(),
        1,
        add_input("zz", "staircase", None),
        &stamp(),
    ));
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert_eq!(err.message, "arm 'zz' is not in the protocol of exp");
    assert_eq!(err.details.get("arms"), Some(&json!(["a"])));
}

// covers: C079
#[test]
fn add_lists_the_phases_when_the_phase_is_unknown() {
    let experiment = running(json!({"min": 1, "max": 3}));
    let err = error_of(trial::add(
        &experiment,
        &task(),
        1,
        add_input("a", "warmup", None),
        &stamp(),
    ));
    assert_eq!(
        err.message,
        "phase 'warmup' is not one of: staircase, refine, control, repeat, live"
    );
}

// covers: C079
#[test]
fn add_refuses_a_band_outside_the_protocol_range() {
    let experiment = running(json!({"min": 1, "max": 3}));
    let err = error_of(trial::add(
        &experiment,
        &task(),
        1,
        add_input("a", "staircase", Some(9)),
        &stamp(),
    ));
    assert_eq!(err.message, "band 9 is outside the protocol range 1..3");
    assert_eq!(err.details.get("max"), Some(&json!(3)));
}

// covers: C079
#[test]
fn add_plans_a_planned_trial_with_one_event() {
    let experiment = running(json!({"min": 1, "max": 3}));
    let decision = match trial::add(
        &experiment,
        &task(),
        2,
        add_input("a", "staircase", Some(2)),
        &stamp(),
    ) {
        Ok(decision) => decision,
        Err(err) => panic!("add refused: {err}"),
    };
    assert_eq!(s(get(&decision.reply, "seq")), "2");
    assert_eq!(s(get(&decision.reply, "status")), "planned");
    assert_eq!(s(get(&decision.reply, "task_key")), "WG-7");
    assert_eq!(s(get(&decision.reply, "requested_model")), "m1");
    assert_eq!(decision.writes.len(), 1);
    assert_eq!(decision.events.len(), 1);
    assert!(matches!(
        decision.writes.first().map(|write| write.key()),
        Some(RowKey::ExperimentTrial(_))
    ));
    assert!(matches!(
        decision.writes.first(),
        Some(write) if matches!(write.table(), wg_model::Table::ExperimentTrial)
    ));
}

// covers: C080
#[test]
fn classify_refuses_a_trial_that_has_not_failed() {
    let experiment = running(json!({"min": 1, "max": 3}));
    let planned = trial("planned", None);
    let err = error_of(trial::classify(
        &experiment,
        &planned,
        "WG-7",
        "too_big",
        "human",
        None,
        &stamp(),
    ));
    assert_eq!(err.code, ErrorCode::Conflict);
    assert_eq!(
        err.message,
        format!(
            "trial {} is planned; only failed or invalid trials are classified",
            id(3)
        )
    );
}

// covers: C080
#[test]
fn classify_names_a_failed_trial_and_keeps_the_notes() {
    let experiment = running(json!({"min": 1, "max": 3}));
    let failed = trial("graded", Some("fail"));
    let decision = match trial::classify(
        &experiment,
        &failed,
        "WG-7",
        "too_big",
        "human",
        None,
        &stamp(),
    ) {
        Ok(decision) => decision,
        Err(err) => panic!("classify refused: {err}"),
    };
    assert_eq!(s(get(&decision.reply, "failure_cause")), "too_big");
    assert_eq!(s(get(&decision.reply, "failure_cause_source")), "human");
    assert_eq!(s(get(&decision.reply, "notes")), "kept");
    assert_eq!(decision.events.len(), 1);
}

// covers: C080
#[test]
fn classify_lists_the_causes_when_the_cause_is_unknown() {
    let experiment = running(json!({"min": 1, "max": 3}));
    let failed = trial("graded", Some("fail"));
    let err = error_of(trial::classify(
        &experiment,
        &failed,
        "WG-7",
        "bogus",
        "human",
        None,
        &stamp(),
    ));
    assert_eq!(
        err.message,
        "failure cause 'bogus' is not one of: too_big, missing_context, ambiguous_spec, coupling, gaming, gate_gap, harness, budget, wrong_model, other"
    );
}
