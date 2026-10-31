//! GV-02: every case under `golden/signatures/extract.json` must produce the recorded signatures
//! for each parser (`auto`, `cargo`, `generic`, `pytest`).
#![allow(clippy::expect_used)]

use serde_json::Value;
use wg_verify::{FailureParser, extract_failure_signatures};

fn load(name: &str) -> Value {
    let path = format!(
        "{}/../../golden/signatures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).expect("golden file is readable");
    serde_json::from_str(&text).expect("golden file is JSON")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("a list of strings")
        .iter()
        .map(|item| item.as_str().expect("a string").to_string())
        .collect()
}

#[test]
fn every_extract_case_matches_its_recorded_signatures() {
    let golden = load("extract.json");
    let cases = golden["cases"].as_array().expect("cases list");
    assert!(!cases.is_empty(), "the vector file has cases");
    for case in cases {
        let id = case["id"].as_str().expect("case id");
        let lines = strings(&case["lines"]);
        for parser in ["auto", "cargo", "generic", "pytest"] {
            let wanted = FailureParser::parse(parser).expect("known parser");
            let got = extract_failure_signatures(&lines, wanted);
            let expected = strings(&case["expected"][parser]);
            assert_eq!(got, expected, "case {id}, parser {parser}");
        }
    }
}
