//! GV-07: the hook payload validation must match Pydantic's `HookPayload` on every golden case. The
//! `validation_cases` hold Pydantic's coerced fields or its error list; every hook case's raw object must
//! validate to the `validated` object the generator recorded.

use serde_json::{Map, Value, json};
use wg_telemetryspec::validate_hook_payload;

const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../golden/telemetry/hook_payload.json"
);

fn load() -> Value {
    let text =
        std::fs::read_to_string(GOLDEN).unwrap_or_else(|error| panic!("reading {GOLDEN}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parsing {GOLDEN}: {error}"))
}

fn cases(document: &Value, key: &str) -> Vec<Value> {
    document[key].as_array().cloned().unwrap_or_default()
}

/// The hook object of a payload, as `process_record` wraps it: `Invalid` with the original under `original`.
fn hook_object(payload: &Value) -> Map<String, Value> {
    match payload.as_object() {
        Some(map) if map.contains_key("hook_event_name") => map.clone(),
        _ => json!({"hook_event_name": "Invalid", "original": payload})
            .as_object()
            .cloned()
            .unwrap_or_default(),
    }
}

// covers: P01
#[test]
fn gv07_validation_matches_pydantic_outcomes() {
    let document = load();
    for case in cases(&document, "validation_cases") {
        let id = case["id"].as_str().unwrap_or("");
        let raw = case["raw"].as_object().cloned().unwrap_or_default();
        match validate_hook_payload(&raw) {
            Ok(validated) => {
                assert_eq!(
                    case["outcome"].as_str(),
                    Some("ok"),
                    "case {id} validated but Pydantic rejected it"
                );
                assert_eq!(
                    validated, case["validated"],
                    "case {id} differs from Pydantic"
                );
            }
            Err(errors) => {
                assert_eq!(
                    case["outcome"].as_str(),
                    Some("error"),
                    "case {id} was rejected but Pydantic accepted it: {errors:?}"
                );
                let actual: Vec<Value> = errors.iter().map(|error| error.to_json()).collect();
                assert_eq!(
                    json!(actual),
                    case["errors"],
                    "case {id} differs from Pydantic"
                );
            }
        }
    }
}

// covers: P01
#[test]
fn gv07_every_hook_case_validates_to_the_recorded_fields() {
    let document = load();
    for case in cases(&document, "cases") {
        let id = case["id"].as_str().unwrap_or("");
        let raw = hook_object(&case["record"]["payload"]);
        let validated = validate_hook_payload(&raw)
            .unwrap_or_else(|errors| panic!("case {id} rejected: {errors:?}"));
        assert_eq!(
            validated, case["validated"],
            "case {id} differs from Pydantic"
        );
    }
}
