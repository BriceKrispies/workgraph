//! GV-04 (attribution): every case of `golden/attribution/` must produce the recorded candidates,
//! both the trailer candidates and the message-inference candidates.

use serde_json::{Value, json};
use wg_gitcore::{Candidate, Evidence, Trailer, message_candidates, trailer_candidates};

fn load(name: &str) -> Value {
    let path = format!(
        "{}/../../golden/attribution/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn cases(golden: &Value) -> Vec<Value> {
    let list = golden["cases"].as_array().cloned().unwrap_or_default();
    assert!(!list.is_empty(), "golden has no cases");
    list
}

fn trailers_of(input: &Value) -> Vec<Trailer> {
    input["trailers"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|t| Trailer {
                    key: t["key"].as_str().unwrap_or_default().to_owned(),
                    value: t["value"].as_str().unwrap_or_default().to_owned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn candidate_json(candidate: &Candidate) -> Value {
    let evidence = match &candidate.evidence {
        Evidence::Trailer { trailer } => json!({ "trailer": trailer }),
        Evidence::Message {
            label,
            matched,
            ambiguous,
            all_matches,
        } => json!({
            "label": label,
            "matched": matched,
            "ambiguous": ambiguous,
            "all_matches": all_matches,
        }),
    };
    json!({
        "task_ref": candidate.task_ref,
        "source": candidate.source.as_str(),
        "confidence": candidate.confidence,
        "evidence": evidence,
    })
}

#[test]
fn gv04_trailer_vectors() {
    let golden = load("trailers");
    let mut failures = Vec::new();
    for case in cases(&golden) {
        let id = case["id"].as_str().unwrap_or("?");
        let trailers = trailers_of(&case["input"]);
        let got: Vec<Value> = trailer_candidates(&trailers)
            .iter()
            .map(candidate_json)
            .collect();
        if Value::Array(got.clone()) != case["output"] {
            failures.push(format!("{id}: got {got:?} want {}", case["output"]));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn gv04_message_vectors() {
    let golden = load("message_inference");
    let mut failures = Vec::new();
    for case in cases(&golden) {
        let id = case["id"].as_str().unwrap_or("?");
        let input = &case["input"];
        let message = input["message"].as_str().unwrap_or_default();
        let prefixes: Vec<&str> = input["prefixes"]
            .as_array()
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let trailers = trailers_of(input);
        let got: Vec<Value> = message_candidates(message, &prefixes, &trailers)
            .iter()
            .map(candidate_json)
            .collect();
        if Value::Array(got.clone()) != case["output"] {
            failures.push(format!("{id}: got {got:?} want {}", case["output"]));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
