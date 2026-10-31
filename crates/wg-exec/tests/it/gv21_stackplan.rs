//! GV-21 for stack base selection: every `builds_on_others` case of `golden/stackplan/` must match
//! `wg_exec::stack_base::choose_sibling`.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use wg_exec::stack_base::{Sibling, SiblingChoice, choose_sibling};

fn load() -> Value {
    let path = format!(
        "{}/../../golden/stackplan/builds_on_others.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn parents_of(case: &Value) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    if let Some(commits) = case["commits"].as_object() {
        for (commit, parents) in commits {
            let list = parents
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            out.insert(commit.clone(), list);
        }
    }
    out
}

fn siblings_of(case: &Value) -> Vec<Sibling> {
    let Some(list) = case["siblings"].as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|s| {
            Some(Sibling {
                key: s["key"].as_str()?.to_owned(),
                number: s["number"].as_i64()?,
                head: s["head"].as_str().map(str::to_owned),
            })
        })
        .collect()
}

/// The golden `result` shape of a choice.
fn as_golden(choice: &SiblingChoice) -> Value {
    match choice {
        SiblingChoice::StackBase => json!({ "source": "stack_base" }),
        SiblingChoice::SiblingHead {
            sibling,
            sibling_head,
            superseded,
        } => json!({
            "source": "sibling_head",
            "sibling": sibling,
            "sibling_head": sibling_head,
            "superseded": superseded
                .iter()
                .map(|s| json!({ "task": s.task, "head": s.head }))
                .collect::<Vec<_>>(),
        }),
        SiblingChoice::Diverged { first, second } => json!({
            "source": "error",
            "error": "ValidationFailed",
            "siblings": [first, second],
        }),
    }
}

#[test]
fn builds_on_others_matches_every_golden_case() {
    let golden = load();
    let cases = golden["cases"].as_array().expect("cases array");
    assert!(!cases.is_empty(), "golden file has no cases");
    for case in cases {
        let got = as_golden(&choose_sibling(&siblings_of(case), &parents_of(case)));
        assert_eq!(got, case["result"], "case {}", case["id"]);
    }
}
