//! GV-01 for attempts: every attempt case of `golden/lifecycle/*.json` must match the attempt
//! state machine re-exported by `wg_exec::attempt`.

use serde_json::Value;
use wg_exec::attempt::{attempt_targets, check_attempt_transition};
use wg_types::{AttemptStatus, WgError};

fn load(name: &str) -> Value {
    let path = format!(
        "{}/../../golden/lifecycle/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn parse_status(value: &Value) -> AttemptStatus {
    let text = value.as_str().unwrap_or_default();
    text.parse()
        .unwrap_or_else(|_| panic!("unknown attempt status {text:?}"))
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
fn attempt_transition_vectors_match() {
    let doc = load("attempt_transitions");
    let vectors = doc["vectors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        vectors.len(),
        81,
        "every (from, to) pair of attempt statuses"
    );
    for case in &vectors {
        let from = parse_status(&case["from"]);
        let to = parse_status(&case["to"]);
        assert_outcome(case, check_attempt_transition(from, to));
    }
}

#[test]
fn attempt_target_tables_match() {
    let doc = load("constants");
    for status in AttemptStatus::ALL {
        let golden = golden_list(&doc["attempt_transitions"][status.as_str()]);
        assert_eq!(
            wire_sorted(attempt_targets(*status)),
            golden,
            "attempt targets of {status}"
        );
    }
}
