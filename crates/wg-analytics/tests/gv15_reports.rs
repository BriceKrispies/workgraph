//! GV-15, reports: the overview, compare, task history and coupling of `analytics/service.py` and
//! `analytics/coupling.py` must return the values the Python service returned for the demo and the
//! generated databases (`golden/analytics/reports-*.json`, recorded by `tools/golden/gen_analytics.py`).
//! Values are compared exactly: integers and floats alike, and rows in the order Python returns them,
//! except where Python orders by a UUID or by hash-set iteration. The golden labels replace those UUIDs,
//! so those lists are compared as multisets (`canonical_overview`, `canonical_coupling`).

mod common;

use std::collections::BTreeMap;

use common::{golden, input, records, text};
use serde_json::Value;
use wg_analytics::build;
use wg_analytics::core::coupling::coupling_report;
use wg_analytics::core::period::Period;
use wg_analytics::core::service::{compare, overview, task_history};
use wg_types::Timestamp;

/// The workspace id of a golden database: the id label of its first workspace row.
fn workspace_of(source: &Value) -> String {
    text(&records(source, "workspaces")[0], "id").unwrap_or_else(|| panic!("workspace id"))
}

fn at(moment: &str) -> Timestamp {
    Timestamp::parse_iso(moment).unwrap_or_else(|err| panic!("{moment}: {err}"))
}

/// The path of the first difference between two values, for the failure message.
fn first_difference(path: &str, actual: &Value, expected: &Value) -> Option<String> {
    match (actual, expected) {
        (Value::Object(left), Value::Object(right)) => {
            if left.len() != right.len() {
                return Some(format!(
                    "{path}: keys {:?} != {:?}",
                    left.keys(),
                    right.keys()
                ));
            }
            left.iter().find_map(|(key, value)| {
                let other = right.get(key)?;
                first_difference(&format!("{path}.{key}"), value, other)
            })
        }
        (Value::Array(left), Value::Array(right)) => {
            if left.len() != right.len() {
                return Some(format!(
                    "{path}: {} items != {} items",
                    left.len(),
                    right.len()
                ));
            }
            left.iter()
                .zip(right)
                .enumerate()
                .find_map(|(index, (a, b))| first_difference(&format!("{path}[{index}]"), a, b))
        }
        _ if actual == expected => None,
        _ => Some(format!("{path}: {actual} != {expected}")),
    }
}

fn assert_equal(label: &str, actual: &Value, expected: &Value) {
    if let Some(difference) = first_difference(label, actual, expected) {
        panic!("{difference}");
    }
}

/// `value` as text with every object's keys sorted, so that objects with the same entries compare
/// equal whatever order their keys were written in.
fn canonical_text(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let entries: BTreeMap<&String, String> = map
                .iter()
                .map(|(key, value)| (key, canonical_text(value)))
                .collect();
            let body: Vec<String> = entries
                .iter()
                .map(|(key, value)| format!("{}:{value}", Value::from(key.as_str())))
                .collect();
            format!("{{{}}}", body.join(","))
        }
        Value::Array(items) => {
            let body: Vec<String> = items.iter().map(canonical_text).collect();
            format!("[{}]", body.join(","))
        }
        other => other.to_string(),
    }
}

/// The items of a list in a canonical order, so that the list compares as a multiset.
fn unordered(items: &Value) -> Value {
    let mut sorted: Vec<Value> = items.as_array().cloned().unwrap_or_default();
    sorted.sort_by_key(canonical_text);
    Value::Array(sorted)
}

/// The overview with the groups of repositories as multisets: Python orders them by repository UUID,
/// which the labelled golden cannot reproduce. Every other list keeps its order.
fn canonical_overview(bundle: &Value) -> Value {
    let lists: Vec<(String, Value)> = bundle
        .as_object()
        .into_iter()
        .flat_map(|map| map.iter())
        .map(|(key, value)| {
            let by_repository = value
                .as_array()
                .into_iter()
                .flatten()
                .any(|row| row.get("repository").is_some());
            let value = if by_repository {
                unordered(value)
            } else {
                value.clone()
            };
            (key.clone(), value)
        })
        .collect();
    Value::Object(lists.into_iter().collect())
}

/// `overview` for every period of the golden, in the golden's order.
fn check_overview(name: &str) {
    let source = golden(&format!("input-{name}.json"));
    let expected = golden(&format!("reports-{name}.json"));
    let warehouse = input(&source);
    let facts = build(&warehouse);
    let ws = workspace_of(&source);
    let cases = expected["reports"]["overview"]
        .as_object()
        .unwrap_or_else(|| panic!("overview"));
    for (period, golden_bundle) in cases {
        let period = Period::parse(period).unwrap_or_else(|| panic!("period {period}"));
        let actual = overview(&warehouse, &facts, &ws, period)
            .unwrap_or_else(|err| panic!("overview {period:?}: {err}"));
        assert_equal(
            &format!("{name}.overview.{}", period.name()),
            &canonical_overview(&actual),
            &canonical_overview(golden_bundle),
        );
    }
}

// covers: C011
#[test]
fn gv15_demo_overview_matches_the_python_service() {
    check_overview("demo");
}

// covers: C011
#[test]
fn gv15_generated_overview_matches_the_python_service() {
    check_overview("gen");
}

/// `compare` over the two windows of the Python report: 180 days from the epoch, then to day 400.
fn check_compare(name: &str) {
    let source = golden(&format!("input-{name}.json"));
    let expected = golden(&format!("reports-{name}.json"));
    let warehouse = input(&source);
    let facts = build(&warehouse);
    let ws = workspace_of(&source);
    let first = (
        at("2026-01-05T09:00:00+00:00"),
        at("2026-07-04T09:00:00+00:00"),
    );
    let second = (
        at("2026-07-04T09:00:00+00:00"),
        at("2027-02-09T09:00:00+00:00"),
    );
    let cases = [
        (
            "tasks_created_by_category",
            "tasks_created",
            vec!["category"],
        ),
        ("execution_duration", "execution_duration", vec![]),
        ("cumulative_churn", "cumulative_churn", vec![]),
    ];
    for (key, metric, group_by) in cases {
        let actual = compare(&warehouse, &facts, &ws, metric, first, second, &group_by)
            .unwrap_or_else(|err| panic!("compare {metric}: {err}"));
        assert_equal(
            &format!("{name}.compare.{key}"),
            &actual,
            &expected["reports"]["compare"][key],
        );
    }
}

// covers: C004
#[test]
fn gv15_demo_compare_matches_the_python_service() {
    check_compare("demo");
}

// covers: C004
#[test]
fn gv15_generated_compare_matches_the_python_service() {
    check_compare("gen");
}

fn check_history(name: &str) {
    let source = golden(&format!("input-{name}.json"));
    let expected = golden(&format!("reports-{name}.json"));
    let warehouse = input(&source);
    let facts = build(&warehouse);
    let ws = workspace_of(&source);
    let actual = task_history(&warehouse, &facts, &ws, None, None, 50);
    assert_equal(
        &format!("{name}.task_history"),
        &actual,
        &expected["reports"]["task_history"],
    );
}

// covers: C009
#[test]
fn gv15_demo_task_history_matches_the_python_service() {
    check_history("demo");
}

// covers: C009
#[test]
fn gv15_generated_task_history_matches_the_python_service() {
    check_history("gen");
}

/// An edge with its two repositories in name order. Python orders the two ends by repository UUID.
fn oriented(edge: &Value) -> Value {
    let mut edge = edge.clone();
    if edge["source_name"].as_str() > edge["target_name"].as_str() {
        let (source, target) = (edge["source"].clone(), edge["target"].clone());
        let (source_name, target_name) = (edge["source_name"].clone(), edge["target_name"].clone());
        edge["source"] = target;
        edge["target"] = source;
        edge["source_name"] = target_name;
        edge["target_name"] = source_name;
    }
    edge
}

/// A list of edges or pairs in name orientation, as a multiset.
fn oriented_list(items: &Value) -> Value {
    unordered(&Value::Array(
        items
            .as_array()
            .into_iter()
            .flatten()
            .map(oriented)
            .collect(),
    ))
}

/// The coupling report with each repository pair in name orientation, the tied lists as multisets
/// and each commit list as a multiset: Python orders the ends by UUID, the ties by hash-set
/// iteration, and the shas of a task by its join order, none of which the labelled golden keeps.
/// Everything else (the matrix, the summary, the repositories and their order) is compared exactly.
fn canonical_coupling(report: &Value) -> Value {
    let mut report = report.clone();
    let edges: Vec<Value> = report["edges"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|edge| {
            let mut edge = oriented(edge);
            let evidence: Vec<Value> = edge["evidence"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| {
                    let mut item = item.clone();
                    let commits: Vec<(String, Value)> = item["commits"]
                        .as_object()
                        .into_iter()
                        .flat_map(|map| map.iter())
                        .map(|(repository, shas)| (repository.clone(), unordered(shas)))
                        .collect();
                    item["commits"] = Value::Object(commits.into_iter().collect());
                    item
                })
                .collect();
            edge["evidence"] = unordered(&Value::Array(evidence));
            edge
        })
        .collect();
    report["edges"] = unordered(&Value::Array(edges));
    report["declared_only"] = oriented_list(&report["declared_only"]);
    report["coupling_candidates"] = oriented_list(&report["coupling_candidates"]);
    report["commonly_changed_pairs"] = unordered(&Value::Array(
        report["commonly_changed_pairs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|pair| {
                let names = pair["pair"].as_array().cloned().unwrap_or_default();
                let (left, right) = (names[0].clone(), names[1].clone());
                let mut pair = pair.clone();
                pair["pair"] = if left.as_str() > right.as_str() {
                    Value::Array(vec![right, left])
                } else {
                    Value::Array(vec![left, right])
                };
                pair
            })
            .collect(),
    ));
    report["modules_with_cross_repo_impact"] = unordered(&Value::Array(
        report["modules_with_cross_repo_impact"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|module| {
                let mut module = module.clone();
                module["tasks"] = unordered(&module["tasks"]);
                module
            })
            .collect(),
    ));
    report
}

fn check_coupling(name: &str) {
    let source = golden(&format!("input-{name}.json"));
    let expected = golden(&format!("reports-{name}.json"));
    let warehouse = input(&source);
    let facts = build(&warehouse);
    let ws = workspace_of(&source);
    for (key, min_support) in [("default", 1), ("min_support_2", 2)] {
        let actual = coupling_report(&warehouse, &facts, &ws, min_support, None);
        assert_equal(
            &format!("{name}.coupling.{key}"),
            &canonical_coupling(&actual),
            &canonical_coupling(&expected["reports"]["coupling"][key]),
        );
    }
}

// covers: C005
#[test]
fn gv15_demo_coupling_matches_the_python_service() {
    check_coupling("demo");
}

// covers: C005
#[test]
fn gv15_generated_coupling_matches_the_python_service() {
    check_coupling("gen");
}
