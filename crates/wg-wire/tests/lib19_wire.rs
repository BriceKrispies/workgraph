//! LIB-19: the experiment, trial, predictor and contract-check DTOs and the experiment MCP output
//! shapes. The field orders are checked against the recorded MCP outputs in
//! `contracts/mcp/stdio.jsonl`, and the timestamp form against what Pydantic prints (`Z`).

#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value as Json, json};
use wg_json::{PyStr, Value, http_json};
use wg_model::entity::{Experiment, ExperimentTrial, PredictorFit};
use wg_types::{Id, Timestamp, parse_id};
use wg_wire::dto::contract_check::{self, ContractVerdict};
use wg_wire::dto::experiment::{self, NAME as EXPERIMENT_NAME};
use wg_wire::dto::experiment_shapes::{self, ArmReport};
use wg_wire::dto::predictor::{self, Prediction};
use wg_wire::dto::trial;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn id_of(text: &str) -> Id {
    parse_id(text).expect("id")
}

fn at(micros: u32) -> Timestamp {
    Timestamp::from_components(2026, 10, 26, 3, 32, 46, micros).expect("timestamp")
}

/// A dict key as text (keys are stored as code points).
fn key_text(key: &PyStr) -> String {
    key.code_points()
        .iter()
        .map(|cp| char::from_u32(*cp).expect("code point"))
        .collect()
}

/// The field names of a model or a dict, in order.
fn names(value: &Value) -> Vec<String> {
    match value {
        Value::Model { fields, .. } => fields.iter().map(|(k, _)| k.clone()).collect(),
        Value::Dict(fields) => fields.iter().map(|(k, _)| key_text(k)).collect(),
        other => panic!("neither a model nor a dict: {other:?}"),
    }
}

fn object(value: Json) -> Map<String, Json> {
    value.as_object().expect("object").clone()
}

fn sample_experiment() -> Experiment {
    Experiment {
        id: id_of("00000000-0000-4000-8000-0000000000e1"),
        workspace_id: id_of("00000000-0000-4000-8000-000000000001"),
        slug: "cli-exp".into(),
        title: "CLI experiment".into(),
        question: Some("Does it?".into()),
        status: "running".into(),
        protocol: object(json!({
            "arms": [{"model": "claude-haiku-5-5", "name": "haiku"}],
            "bands": {"min": 1, "max": 5},
        })),
        frozen_at: Some(at(674_635)),
        created_at: at(166_931),
        updated_at: at(674_635),
    }
}

fn sample_trial() -> ExperimentTrial {
    ExperimentTrial {
        id: id_of("00000000-0000-4000-8000-0000000000f1"),
        experiment_id: id_of("00000000-0000-4000-8000-0000000000e1"),
        task_id: id_of("00000000-0000-4000-8000-0000000000c1"),
        attempt_id: None,
        subagent_run_id: None,
        arm: "haiku".into(),
        requested_model: "claude-haiku-5-5".into(),
        actual_model: None,
        phase: "staircase".into(),
        seq: 1,
        band: Some(5),
        features: object(json!({"predicted": {"replay": {"src_lines": 33}}})),
        base_ref: None,
        worktree_path: None,
        status: "planned".into(),
        outcome: None,
        outcome_detail: object(json!({})),
        failure_cause: None,
        failure_cause_source: None,
        notes: None,
        created_at: at(10_009),
        started_at: None,
        graded_at: None,
    }
}

fn sample_fit() -> PredictorFit {
    PredictorFit {
        id: id_of("00000000-0000-4000-8000-0000000000d1"),
        workspace_id: id_of("00000000-0000-4000-8000-000000000001"),
        model: "claude-haiku-5-5".into(),
        created_at: at(385_546),
        data_cutoff: Some(at(261_328)),
        trial_ids: ["t1", "t2"].into_iter().map(String::from).collect(),
        feature_names: ["log2_src_lines"].into_iter().map(String::from).collect(),
        repositories: ["mcp-replay"].into_iter().map(String::from).collect(),
        coef: [json!(6.0), json!(-1.0)].into_iter().collect(),
        penalty: [json!(0.0), json!(0.01)].into_iter().collect(),
        n: 5,
        converged: true,
        metrics: object(json!({"brier": 0.25, "n_pass": 3, "n_fail": 2})),
        samples: [json!([6.0, -1.0]), json!([7.0, -2.0])]
            .into_iter()
            .collect(),
    }
}

fn sample_prediction() -> Prediction {
    Prediction {
        probability: 0.369_735,
        low: Some(0.000_198),
        high: Some(0.854_735),
        fit_id: id_of("00000000-0000-4000-8000-0000000000d1"),
        model: "claude-haiku-5-5".into(),
        data_cutoff: Some(at(261_328)),
        n: 5,
        repository: Some("mcp-replay".into()),
        repository_known: true,
    }
}

/// The `output` of the recorded MCP tool call with the given label, parsed.
fn recorded_mcp_output(label: &str) -> Json {
    let text = fs::read_to_string(root().join("contracts/mcp/stdio.jsonl")).expect("stdio.jsonl");
    let record = text
        .lines()
        .map(|line| serde_json::from_str::<Json>(line).expect("jsonl record"))
        .find(|record| record["label"] == label)
        .unwrap_or_else(|| panic!("no recording labelled {label}"));
    let body = record["response"]["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    serde_json::from_str(body).expect("tool output is JSON")
}

fn keys(value: &Json) -> Vec<String> {
    value.as_object().expect("object").keys().cloned().collect()
}

fn texts(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn experiment_and_trial_field_order_matches_the_recorded_outputs() {
    let experiment_value = experiment::from_model(&sample_experiment());
    assert_eq!(names(&experiment_value), experiment::FIELDS);
    assert_eq!(EXPERIMENT_NAME, "ExperimentOut");

    let trial_value = trial::from_model(&sample_trial(), "DEMO-16");
    assert_eq!(names(&trial_value), trial::FIELDS);

    let listed = recorded_mcp_output("experiment_list:ok");
    assert_eq!(keys(&listed["experiments"][0]), texts(&experiment::FIELDS));

    let got = recorded_mcp_output("experiment_get:ok");
    assert_eq!(keys(&got["experiment"]), texts(&experiment::FIELDS));
    assert_eq!(keys(&got["trials"][0]), texts(&trial::FIELDS));

    for label in ["trial_start:ok", "trial_grade:ok", "trial_classify:ok"] {
        let output = recorded_mcp_output(label);
        let trial_output = if label == "trial_start:ok" {
            &output["trial"]
        } else {
            &output
        };
        assert_eq!(keys(trial_output), texts(&trial::FIELDS), "{label}");
    }
}

#[test]
fn experiment_and_trial_timestamps_print_in_the_z_form() {
    let text = http_json(&experiment::from_model(&sample_experiment())).expect("dump");
    assert!(
        text.contains(r#""created_at":"2026-10-26T03:32:46.166931Z""#),
        "{text}"
    );
    assert!(text.contains(r#""question":"Does it?""#), "{text}");

    let text = http_json(&trial::from_model(&sample_trial(), "DEMO-16")).expect("dump");
    assert!(text.contains(r#""task_key":"DEMO-16""#), "{text}");
    assert!(text.contains(r#""started_at":null"#), "{text}");
    assert!(text.contains(r#""band":5"#), "{text}");
    assert!(
        text.contains(r#""created_at":"2026-10-26T03:32:46.010009Z""#),
        "{text}"
    );

    let recorded = recorded_mcp_output("trial_start:ok");
    assert_eq!(
        recorded["trial"]["created_at"],
        json!("2026-10-26T03:34:29.010009Z")
    );
}

#[test]
fn predictor_fit_and_prediction_fields_match_the_recorded_outputs() {
    let fit = predictor::fit_from_model(&sample_fit());
    assert_eq!(names(&fit), predictor::FIT_FIELDS);
    let text = http_json(&fit).expect("dump");
    assert!(text.contains(r#""coef":[6.0,-1.0]"#), "{text}");
    assert!(
        text.contains(r#""samples":[[6.0,-1.0],[7.0,-2.0]]"#),
        "{text}"
    );
    assert!(
        text.contains(r#""data_cutoff":"2026-10-26T03:32:46.261328Z""#),
        "{text}"
    );

    let recorded = recorded_mcp_output("experiment_fit:ok");
    assert_eq!(keys(&recorded), texts(&predictor::FIT_FIELDS));

    let prediction = predictor::prediction(&sample_prediction());
    assert_eq!(names(&prediction), predictor::PREDICTION_FIELDS);
    let text = http_json(&prediction).expect("dump");
    assert!(text.contains(r#""inference":true"#), "{text}");
    assert!(text.contains(r#""repository_known":true"#), "{text}");

    let recorded = recorded_mcp_output("experiment_predict:ok");
    assert_eq!(keys(&recorded), texts(&predictor::PREDICTION_FIELDS));
}

#[test]
fn contract_verdict_dict_matches_the_recorded_output() {
    let verdict = ContractVerdict {
        verdict: "incomplete".into(),
        reasons: vec!["contract is missing or malformed: fix the fields listed in missing".into()],
        missing: vec!["contract.files".into()],
        prediction: None,
        target: 0.8,
        model: "claude-haiku-5-5".into(),
    };
    let value = contract_check::verdict(&verdict);
    let expected = [
        "verdict",
        "reasons",
        "missing",
        "prediction",
        "target",
        "model",
    ];
    assert_eq!(names(&value), expected);
    assert_eq!(
        keys(&recorded_mcp_output("task_contract_check:ok")),
        texts(&expected)
    );
    let text = http_json(&value).expect("dump");
    assert!(text.contains(r#""prediction":null"#), "{text}");
    assert!(text.contains(r#""target":0.8"#), "{text}");

    let with_prediction = ContractVerdict {
        prediction: Some(sample_prediction()),
        verdict: "leaf".into(),
        ..verdict
    };
    let text = http_json(&contract_check::verdict(&with_prediction)).expect("dump");
    assert!(
        text.contains(r#""prediction":{"probability":0.369735"#),
        "{text}"
    );
}

#[test]
fn experiment_mcp_shapes_match_the_recorded_outputs() {
    let next = recorded_mcp_output("experiment_next:ok");
    let step = experiment_shapes::step("staircase", Some(5), 0, Some(5.0), &[(5, 1, 2), (5, 1, 2)]);
    assert_eq!(
        names(&step),
        texts(&["phase", "band", "reversals", "threshold", "visits"])
    );
    let planned = experiment_shapes::next_plan(
        false,
        Some("staircase"),
        Some(trial::from_model(&sample_trial(), "DEMO-16")),
        "planned",
        step,
    );
    assert_eq!(
        names(&planned),
        texts(&["done", "phase", "trial", "reason", "step"])
    );
    assert_eq!(
        keys(&next),
        texts(&["done", "phase", "trial", "reason", "step"])
    );
    assert_eq!(
        keys(&next["step"]),
        texts(&["phase", "band", "reversals", "threshold", "visits"])
    );
    let text = http_json(&planned).expect("dump");
    assert!(text.contains(r#""threshold":5.0"#), "{text}");
    assert!(text.contains(r#""visits":[[5,1,2],[5,1,2]]"#), "{text}");

    let start = recorded_mcp_output("trial_start:ok");
    let shaped = experiment_shapes::trial_start(
        trial::from_model(&sample_trial(), "DEMO-16"),
        "C:/worktree",
        "wg/trial/f7c8957f",
        "f1c6b5160a759fc2af0cd403ae361b985fda4963",
        "brief",
    );
    assert_eq!(
        names(&shaped),
        texts(&["trial", "worktree", "branch", "base_sha", "brief"])
    );
    assert_eq!(
        keys(&start),
        texts(&["trial", "worktree", "branch", "base_sha", "brief"])
    );

    let listed = experiment_shapes::experiments(vec![experiment::from_model(&sample_experiment())]);
    assert_eq!(names(&listed), texts(&["experiments"]));
    let got = experiment_shapes::experiment(
        experiment::from_model(&sample_experiment()),
        vec![trial::from_model(&sample_trial(), "DEMO-16")],
    );
    assert_eq!(names(&got), texts(&["experiment", "trials"]));
}

#[test]
fn experiment_report_shape_matches_the_recorded_output() {
    let recorded = recorded_mcp_output("experiment_report:ok");
    let arm = experiment_shapes::arm(&ArmReport {
        arm: "haiku".into(),
        trials: experiment_shapes::trial_counts(
            5,
            &[("graded".into(), 5)],
            &[("staircase".into(), 5)],
        ),
        bands: Value::List(vec![experiment_shapes::band_row(3, 1, 1, 1.0, (0.2, 1.0))]),
        fit: Value::Null,
        fit_reason: Some("too few trials".into()),
        usage: Value::Dict(vec![]),
        failure_causes: vec![("other".into(), 1)],
        invalid: Value::List(vec![]),
    });
    assert_eq!(
        names(&arm),
        texts(&[
            "arm",
            "trials",
            "bands",
            "fit",
            "fit_reason",
            "usage",
            "failure_causes",
            "invalid",
        ])
    );
    assert_eq!(
        keys(&recorded["arms"][0]),
        texts(&[
            "arm",
            "trials",
            "bands",
            "fit",
            "fit_reason",
            "usage",
            "failure_causes",
            "invalid",
        ])
    );

    let report = experiment_shapes::report(
        "cli-exp",
        "running",
        &object(json!({"arms": []})),
        Value::Null,
        vec![arm],
    );
    assert_eq!(names(&report), texts(&["experiment", "step", "arms"]));
    assert_eq!(keys(&recorded), texts(&["experiment", "step", "arms"]));
    assert_eq!(
        keys(&recorded["experiment"]),
        texts(&["slug", "status", "protocol"])
    );
}
