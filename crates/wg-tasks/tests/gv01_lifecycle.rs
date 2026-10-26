//! GV-01: every case of `golden/lifecycle/*.json` must match the lifecycle module's result: the
//! same ok or error, error code, message and details, and the same transition and constant tables.

use serde_json::Value;
use wg_tasks::lifecycle::{
    REQUIRES_DEPENDENCIES, RESTRICTED_TARGETS, TRACKER_CLOSE_TARGETS, attempt_targets,
    check_attempt_transition, check_task_transition, task_targets,
};
use wg_types::{AttemptStatus, TaskStatus, WgError};

fn load(name: &str) -> Value {
    let path = format!(
        "{}/../../golden/lifecycle/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn parse_status<T: std::str::FromStr<Err = WgError>>(value: &Value) -> T {
    let text = value.as_str().unwrap_or_default();
    text.parse()
        .unwrap_or_else(|_| panic!("unknown status {text:?}"))
}

fn wire_sorted<T: std::fmt::Display>(items: &[T]) -> Vec<String> {
    let mut out: Vec<String> = items.iter().map(ToString::to_string).collect();
    out.sort();
    out
}

fn golden_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Compares a vector's recorded outcome with a result of the lifecycle module.
fn assert_outcome(case: &Value, got: Result<(), WgError>) {
    if case["result"] == "ok" {
        assert!(got.is_ok(), "{case}: expected ok, got {got:?}");
        return;
    }
    let err = got
        .err()
        .unwrap_or_else(|| panic!("{case}: expected an error"));
    assert_eq!(
        err.code.as_str(),
        case["code"].as_str().unwrap_or_default(),
        "{case}"
    );
    assert_eq!(
        err.message,
        case["message"].as_str().unwrap_or_default(),
        "{case}"
    );
    assert_eq!(Value::Object(err.details), case["details"], "{case}");
}

#[test]
fn task_transition_vectors_match() {
    let doc = load("task_transitions");
    let vectors = doc["vectors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        vectors.len(),
        400,
        "every (from, to, privileged, tracker) combination"
    );
    for case in &vectors {
        let from: TaskStatus = parse_status(&case["from"]);
        let to: TaskStatus = parse_status(&case["to"]);
        let privileged = case["privileged"].as_bool().unwrap_or(false);
        let tracker = case["tracker"].as_bool().unwrap_or(false);
        assert_outcome(case, check_task_transition(from, to, privileged, tracker));
    }
}

#[test]
fn attempt_transition_vectors_match() {
    let doc = load("attempt_transitions");
    let vectors = doc["vectors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        vectors.len(),
        81,
        "every (from, to) pair of attempt statuses"
    );
    for case in &vectors {
        let from: AttemptStatus = parse_status(&case["from"]);
        let to: AttemptStatus = parse_status(&case["to"]);
        assert_outcome(case, check_attempt_transition(from, to));
    }
}

#[test]
fn constant_tables_match() {
    let doc = load("constants");
    for status in TaskStatus::ALL {
        let golden = golden_list(&doc["task_transitions"][status.as_str()]);
        assert_eq!(
            wire_sorted(task_targets(*status)),
            golden,
            "task targets of {status}"
        );
    }
    for status in AttemptStatus::ALL {
        let golden = golden_list(&doc["attempt_transitions"][status.as_str()]);
        assert_eq!(
            wire_sorted(attempt_targets(*status)),
            golden,
            "attempt targets of {status}"
        );
    }
    assert_eq!(
        wire_sorted(RESTRICTED_TARGETS),
        golden_list(&doc["restricted_targets"])
    );
    assert_eq!(
        wire_sorted(TRACKER_CLOSE_TARGETS),
        golden_list(&doc["tracker_close_targets"])
    );
    assert_eq!(
        wire_sorted(REQUIRES_DEPENDENCIES),
        golden_list(&doc["requires_dependencies"])
    );
}
