//! CR-07: the coverage report reads the contract inventory and joins it with scenario `covers`.

use std::path::PathBuf;

use wg_conformance::coverage::{coverage, inventory_ids, report_json, report_text};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn scenario(id: &str, covers: &[&str]) -> (String, Vec<String>) {
    (
        id.to_owned(),
        covers.iter().map(|c| (*c).to_owned()).collect(),
    )
}

#[test]
fn inventory_of_the_repository_lists_every_family() {
    let path = repo_root().join("docs").join("rust-rewrite-contracts.md");
    let markdown =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let ids = inventory_ids(&markdown);
    // Counts of the inventory's tables: 192 C, 18 E, 15 F, 130 H, 69 M, 15 O, 8 P, 7 X, 12 B.
    assert_eq!(ids.len(), 466);
    for id in [
        "C001", "C192", "E01", "E24", "F01", "H001", "H130", "M01", "M69", "O15", "X7",
    ] {
        assert!(ids.iter().any(|i| i == id), "missing {id}");
    }
    assert!(!ids.iter().any(|i| i == "#"), "header cells are not ids");
}

#[test]
fn inventory_takes_first_cells_of_table_rows_only() {
    let markdown = "\
| # | Command |
| --- | --- |
| C001 | `a` |
| **M02** | `b` |
| C001 | duplicate row |
Prose that mentions C009 and | not a row.
| x1 | lower case is not an id |
| 1 | digits only are not an id |
";
    assert_eq!(inventory_ids(markdown), vec!["C001", "M02"]);
}

#[test]
fn coverage_lists_scenarios_per_id_and_uncovered_ids() {
    let inventory: Vec<String> = ["C001", "C002", "M01"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let scenarios = [
        scenario("alpha", &["C001", "M01"]),
        scenario("beta", &["C001", "C001"]),
    ];
    let c = coverage(&inventory, &scenarios);
    assert_eq!(c.by_id["C001"], vec!["alpha", "beta"]);
    assert_eq!(c.by_id["M01"], vec!["alpha"]);
    assert!(c.by_id["C002"].is_empty());
    assert_eq!(c.uncovered(), vec!["C002"]);
    assert!(c.unknown.is_empty());
}

#[test]
fn ids_outside_the_inventory_are_reported_as_unknown() {
    let inventory = vec!["C001".to_owned()];
    let c = coverage(&inventory, &[scenario("gamma", &["C001", "Z9"])]);
    assert_eq!(c.unknown["Z9"], vec!["gamma"]);
    assert!(!c.by_id.contains_key("Z9"));
}

#[test]
fn empty_corpus_leaves_every_id_uncovered() {
    let inventory: Vec<String> = ["X1", "X2"].iter().map(|s| (*s).to_owned()).collect();
    let c = coverage(&inventory, &[]);
    assert_eq!(c.uncovered(), vec!["X1", "X2"]);
    let text = report_text(&c, &[]);
    assert!(text.starts_with("coverage: 0 of 2 contract ids covered, 2 uncovered\n"));
    assert!(text.contains("uncovered:\n  X1\n  X2\n"));
}

#[test]
fn json_report_matches_text_counts() {
    let inventory: Vec<String> = ["X1", "X2"].iter().map(|s| (*s).to_owned()).collect();
    let c = coverage(&inventory, &[scenario("delta", &["X2"])]);
    let invalid = vec!["broken.json: missing `steps`".to_owned()];
    let value = report_json(&c, &invalid);
    assert_eq!(value["total"], 2);
    assert_eq!(value["covered"], 1);
    assert_eq!(value["uncovered"], serde_json::json!(["X1"]));
    assert_eq!(value["ids"]["X2"], serde_json::json!(["delta"]));
    assert_eq!(value["invalid"], serde_json::json!(invalid));
    let text = report_text(&c, &invalid);
    assert!(text.contains("invalid scenarios (not counted):\n  broken.json: missing `steps`\n"));
}
