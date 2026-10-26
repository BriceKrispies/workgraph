//! GV-09: every case of `golden/intel/*.json` must reproduce the value the Python implementation
//! recorded (or its exception), for the classifier, the heuristic provider, the taxonomy, FTS
//! quoting and the search arithmetic.

use std::fs;
use std::path::PathBuf;

use serde_json::Value;
use wg_intel::{
    FtsOptions, HeuristicError, PROMPT_VERSION, RRF_K, TAXONOMY_VERSION, build_prompt,
    classify_text, cosine, decompose_text, fuse, generate_json, hash_embedding, input_hash,
    narrative_from_facts, to_fts_query, validate_classification,
};

fn load(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/intel")
        .join(format!("{name}.json"));
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn cases<'a>(golden: &'a Value, key: &str) -> &'a [Value] {
    golden
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

fn text_of(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

fn strings_of(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn floats_of(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// JSON equality where every number compares by its `f64` value (Python writes `1` and `1.0`
/// alike when a float is integral).
fn same(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && b.iter()
                    .all(|(key, value)| a.get(key).is_some_and(|x| same(x, value)))
        }
        _ => actual == expected,
    }
}

fn assert_same(context: &str, actual: &Value, expected: &Value) {
    assert!(
        same(actual, expected),
        "{context}:\n  actual   {actual}\n  expected {expected}"
    );
}

#[test]
fn gv09_classify_cases_match_python() {
    let golden = load("classify");
    for case in cases(&golden, "cases") {
        let id = text_of(case, "id");
        let title = text_of(case, "title");
        let description = text_of(case, "description");
        let criteria = strings_of(case, "criteria");
        let tags = strings_of(case, "tags");
        let prompt = build_prompt(&title, &description, &criteria, &tags);
        assert_same(
            &format!("{id} prompt"),
            &Value::String(prompt.clone()),
            &case["prompt"],
        );
        assert_same(
            &format!("{id} text_classify"),
            &classify_text(&format!("{title}\n{description}")),
            &case["text_classify"],
        );
        let provider = generate_json(Some("TaskClassification"), &prompt).unwrap_or(Value::Null);
        assert_same(
            &format!("{id} provider_data"),
            &provider,
            &case["provider_data"],
        );
        let validated = validate_classification(&provider).unwrap_or(Value::Null);
        assert_same(&format!("{id} validated"), &validated, &case["validated"]);
        assert_eq!(
            input_hash(&title, &description, &criteria),
            text_of(case, "input_hash"),
            "{id} input_hash"
        );
    }
}

#[test]
fn gv09_decompose_cases_match_python() {
    let golden = load("decompose");
    for case in cases(&golden, "cases") {
        let id = text_of(case, "id");
        let description = text_of(case, "description");
        let prompt = format!("<task>{description}</task>");
        let provider = generate_json(Some("TaskDecomposition"), &prompt).unwrap_or(Value::Null);
        assert_same(
            &format!("{id} provider_data"),
            &provider,
            &case["provider_data"],
        );
        assert_same(
            &format!("{id} direct"),
            &decompose_text(&description),
            &case["direct"],
        );
    }
}

#[test]
fn gv09_narrative_cases_match_python() {
    let golden = load("narrative");
    for case in cases(&golden, "cases") {
        let id = text_of(case, "id");
        let facts = text_of(case, "facts");
        let outcome = narrative_from_facts(&facts);
        if let Some(expected) = case.get("result") {
            let actual = outcome.unwrap_or_else(|e| panic!("{id}: unexpected error {e}"));
            assert_same(&format!("{id} result"), &actual, expected);
        } else {
            let error = match outcome {
                Ok(value) => panic!("{id}: expected an error, got {value}"),
                Err(error) => error,
            };
            let python_name = match error {
                HeuristicError::NotJson => "InvalidModelOutputError",
                HeuristicError::NotAnObject(_) => "AttributeError",
                other => panic!("{id}: no Python counterpart for {other:?}"),
            };
            let raised = &case["raised"];
            assert_eq!(raised["error"], python_name, "{id} error type");
            assert_eq!(raised["message"], error.to_string(), "{id} message");
        }
    }
}

#[test]
fn gv09_embedding_cases_match_python() {
    let golden = load("embedding");
    let dim = golden["dimensions"].as_u64().unwrap_or(256) as usize;
    for case in cases(&golden, "cases") {
        let id = text_of(case, "id");
        let actual = hash_embedding(&text_of(case, "text"), dim);
        let expected = floats_of(&case["vector"]);
        assert_eq!(actual, expected, "{id} vector");
    }
}

#[test]
fn gv09_taxonomy_cases_match_python() {
    let golden = load("taxonomy");
    assert_eq!(golden["taxonomy_version"], TAXONOMY_VERSION);
    assert_eq!(golden["prompt_version"], PROMPT_VERSION);
    for case in cases(&golden, "prompts") {
        let id = text_of(case, "id");
        let title = text_of(case, "title");
        let description = text_of(case, "description");
        let criteria = strings_of(case, "criteria");
        let tags = strings_of(case, "tags");
        assert_eq!(
            build_prompt(&title, &description, &criteria, &tags),
            text_of(case, "prompt"),
            "{id} prompt"
        );
        assert_eq!(
            input_hash(&title, &description, &criteria),
            text_of(case, "input_hash"),
            "{id} input_hash"
        );
    }
    for case in cases(&golden, "validations") {
        let id = text_of(case, "id");
        let outcome = validate_classification(&case["payload"]);
        if case["valid"] == Value::Bool(true) {
            let value = outcome.unwrap_or_else(|e| panic!("{id}: rejected with {e:?}"));
            assert_same(&format!("{id} value"), &value, &case["value"]);
        } else {
            let errors: Vec<String> = case["errors"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(outcome, Err(errors), "{id} errors");
        }
    }
}

#[test]
fn gv09_fts_cases_match_python() {
    let golden = load("fts");
    let expression = |query: &str, prefix: bool, match_any: bool| {
        to_fts_query(query, FtsOptions { prefix, match_any })
    };
    for case in cases(&golden, "cases") {
        let id = text_of(case, "id");
        let query = text_of(case, "query");
        let expected = [
            ("all", expression(&query, true, false)),
            ("prefix_false", expression(&query, false, false)),
            ("match_any", expression(&query, true, true)),
            ("match_any_no_prefix", expression(&query, false, true)),
        ];
        for (field, actual) in expected {
            let wanted = case[field].as_str().map(str::to_string);
            assert_eq!(actual, wanted, "{id} {field} for {query:?}");
        }
    }
}

#[test]
fn gv09_search_cases_match_python() {
    let golden = load("search");
    assert_eq!(golden["rrf_k"], u64::from(RRF_K));
    for case in cases(&golden, "cosine") {
        let id = text_of(case, "id");
        let a = floats_of(&case["a"]);
        let b = floats_of(&case["b"]);
        let expected = case["cosine"].as_f64().unwrap_or(f64::NAN);
        assert_eq!(cosine(&a, &b), expected, "{id} cosine");
    }
    for case in cases(&golden, "fusion") {
        let id = text_of(case, "id");
        let limit = case["limit"].as_u64().unwrap_or(0) as usize;
        let pairs = |key: &str, value_key: &str| -> Vec<(String, f64)> {
            cases_of_hits(case, key, value_key)
        };
        let lexical = pairs("lexical", "bm25");
        let semantic = pairs("semantic", "similarity");
        let fused = fuse(&lexical, &semantic, limit);
        let expected = cases(case, "fused");
        assert_eq!(fused.len(), expected.len(), "{id} length");
        for (hit, want) in fused.iter().zip(expected) {
            assert_eq!(hit.task_id, text_of(want, "task_id"), "{id} task_id");
            assert_eq!(Some(hit.score), want["score"].as_f64(), "{id} score");
            assert_eq!(
                hit.lexical_rank,
                want["lexical_rank"].as_u64().map(|r| r as usize),
                "{id} lexical_rank"
            );
            assert_eq!(
                hit.semantic_rank,
                want["semantic_rank"].as_u64().map(|r| r as usize),
                "{id} semantic_rank"
            );
        }
    }
}

fn cases_of_hits(case: &Value, key: &str, value_key: &str) -> Vec<(String, f64)> {
    cases(case, key)
        .iter()
        .map(|hit| {
            (
                text_of(hit, "task_id"),
                hit[value_key].as_f64().unwrap_or(f64::NAN),
            )
        })
        .collect()
}
