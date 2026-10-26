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
use wg_analytics::core::failures::failures;
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

/// SHA-256 of `message` as lowercase hex, the digest the Python fingerprints use (FIPS 180-4). The
/// test needs it because the Core crate may not depend on a hash implementation.
fn sha256_hex(message: &[u8]) -> String {
    const ROUND: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((message.len() as u64) * 8).to_be_bytes());
    for block in padded.chunks(64) {
        let mut schedule = [0u32; 64];
        for (word, bytes) in schedule.iter_mut().zip(block.chunks(4)) {
            *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for i in 16..64 {
            let s0 = schedule[i - 15].rotate_right(7)
                ^ schedule[i - 15].rotate_right(18)
                ^ (schedule[i - 15] >> 3);
            let s1 = schedule[i - 2].rotate_right(17)
                ^ schedule[i - 2].rotate_right(19)
                ^ (schedule[i - 2] >> 10);
            schedule[i] = schedule[i - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[i - 7])
                .wrapping_add(s1);
        }
        let mut v = state;
        for (k, w) in ROUND.iter().zip(schedule.iter()) {
            let [a, b, c, d, e, f, g, h] = v;
            let t1 = h
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(*k)
                .wrapping_add(*w);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            v = [t1.wrapping_add(t2), a, b, c, d.wrapping_add(t1), e, f, g];
        }
        state = [
            state[0].wrapping_add(v[0]),
            state[1].wrapping_add(v[1]),
            state[2].wrapping_add(v[2]),
            state[3].wrapping_add(v[3]),
            state[4].wrapping_add(v[4]),
            state[5].wrapping_add(v[5]),
            state[6].wrapping_add(v[6]),
            state[7].wrapping_add(v[7]),
        ];
    }
    state.iter().map(|word| format!("{word:08x}")).collect()
}

#[test]
fn sha256_matches_the_standard_vector() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

/// The failures with two orders normalised. The attempt lists of the `failure_category` findings are
/// multisets (Python gathers them with an unordered `list()` aggregate). Within a run of findings of
/// one kind, the order is a multiset too, where Python's `ORDER BY` leaves ties unordered (retry
/// loops with the same attempt count, for one).
fn canonical_failures(report: &Value) -> Value {
    let mut report = report.clone();
    let findings: Vec<Value> = report["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|finding| {
            let mut finding = finding.clone();
            match finding["kind"].as_str() {
                Some("failure_category") => {
                    finding["evidence"]["attempts"] = unordered(&finding["evidence"]["attempts"]);
                }
                Some("regression_follow_up") => {
                    finding["evidence"]["commits"] = unordered(&finding["evidence"]["commits"]);
                }
                _ => {}
            }
            finding
        })
        .collect();
    let runs: Vec<Value> = findings
        .chunk_by(|a, b| a["kind"] == b["kind"])
        .flat_map(|run| {
            unordered(&Value::Array(run.to_vec()))
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    report["findings"] = Value::Array(runs);
    report
}

/// The failure report. Its fingerprints are SHA-256 digests of `kind|title`, which the analysis takes
/// from its caller; the report's own generation instant is the golden's.
fn check_failures(name: &str) {
    let source = golden(&format!("input-{name}.json"));
    let expected = golden(&format!("reports-{name}.json"));
    let warehouse = input(&source);
    let facts = build(&warehouse);
    let ws = workspace_of(&source);
    let golden_report = &expected["reports"]["failures"];
    let generated_at = golden_report["generated_at"]
        .as_str()
        .unwrap_or_else(|| panic!("generated_at"));
    let digest = |text: &str| sha256_hex(text.as_bytes());
    let actual = failures(&warehouse, &facts, &ws, generated_at, &digest);
    assert_equal(
        &format!("{name}.failures"),
        &canonical_failures(&actual),
        &canonical_failures(golden_report),
    );
}

// covers: C008
#[test]
fn gv15_demo_failures_match_the_python_service() {
    check_failures("demo");
}

// covers: C008
#[test]
fn gv15_generated_failures_match_the_python_service() {
    check_failures("gen");
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
