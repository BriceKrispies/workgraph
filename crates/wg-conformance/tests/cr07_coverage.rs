//! CR-07: the coverage report reads the contract inventory and joins it with scenario `covers`,
//! test `covers` markers and the allowlist of reasoned exclusions.

use std::path::PathBuf;

use wg_conformance::coverage::{
    coverage, inventory_ids, marked_tests, parse_exclusions, report_json, report_text,
};

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

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn source(lines: &[&str]) -> String {
    let mut text = lines.join("\n");
    text.push('\n');
    text
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
fn coverage_lists_sources_per_id_and_uncovered_ids() {
    let inventory = ids(&["C001", "C002", "M01"]);
    let sources = [
        scenario("alpha", &["C001", "M01"]),
        scenario("beta", &["C001", "C001"]),
    ];
    let c = coverage(&inventory, &sources, &[]);
    assert_eq!(c.by_id["C001"], vec!["alpha", "beta"]);
    assert_eq!(c.by_id["M01"], vec!["alpha"]);
    assert!(c.by_id["C002"].is_empty());
    assert_eq!(c.uncovered(), vec!["C002"]);
    assert!(c.unknown.is_empty());
}

#[test]
fn ids_outside_the_inventory_are_reported_as_unknown() {
    let inventory = ids(&["C001"]);
    let c = coverage(&inventory, &[scenario("gamma", &["C001", "Z9"])], &[]);
    assert_eq!(c.unknown["Z9"], vec!["gamma"]);
    assert!(!c.by_id.contains_key("Z9"));
}

#[test]
fn empty_corpus_leaves_every_id_uncovered() {
    let inventory = ids(&["X1", "X2"]);
    let c = coverage(&inventory, &[], &[]);
    assert_eq!(c.uncovered(), vec!["X1", "X2"]);
    let text = report_text(&c, &[]);
    assert!(text.starts_with(
        "coverage: 0 of 2 contract ids covered, 0 excluded with a reason, 2 uncovered\n"
    ));
    assert!(text.contains("uncovered:\n  X1\n  X2\n"));
}

#[test]
fn json_report_matches_text_counts() {
    let inventory = ids(&["X1", "X2"]);
    let c = coverage(&inventory, &[scenario("delta", &["X2"])], &[]);
    let invalid = vec!["broken.json: missing `steps`".to_owned()];
    let value = report_json(&c, &invalid);
    assert_eq!(value["total"], 2);
    assert_eq!(value["covered"], 1);
    assert_eq!(value["uncovered"], serde_json::json!(["X1"]));
    assert_eq!(value["ids"]["X2"], serde_json::json!(["delta"]));
    assert_eq!(value["invalid"], serde_json::json!(invalid));
    let text = report_text(&c, &invalid);
    assert!(text.contains("invalid (not counted):\n  broken.json: missing `steps`\n"));
}

#[test]
fn rust_test_marker_counts_the_test_as_coverage() {
    // Fixtures are arrays of quoted lines so that no source line of this file starts with a marker.
    let text = source(&[
        "use std::fmt;",
        "",
        "// covers: C001, M02",
        "#[test]",
        "fn checks_the_first_command() {",
        "    assert!(true);",
        "}",
    ]);
    let (tests, problems) = marked_tests("crates/x/tests/a.rs", &text);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        tests,
        vec![(
            "crates/x/tests/a.rs::checks_the_first_command".to_owned(),
            ids(&["C001", "M02"])
        )]
    );
    let inventory = ids(&["C001", "M02", "M03"]);
    let c = coverage(&inventory, &tests, &[]);
    assert_eq!(
        c.by_id["M02"],
        vec!["crates/x/tests/a.rs::checks_the_first_command"]
    );
    assert_eq!(c.uncovered(), vec!["M03"]);
}

#[test]
fn python_test_marker_across_decorators_and_doc_comment_lines() {
    let text = source(&[
        "import pytest",
        "",
        "# covers: C002",
        "# a second comment line",
        "@pytest.mark.slow",
        "def test_renders_the_report(tmp_path):",
        "    assert tmp_path",
    ]);
    let (tests, problems) = marked_tests("tests/unit/test_x.py", &text);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        tests,
        vec![(
            "tests/unit/test_x.py::test_renders_the_report".to_owned(),
            ids(&["C002"])
        )]
    );
}

#[test]
fn marker_problems_are_reported_with_their_line() {
    let text = source(&[
        "// covers: C001, not-an-id",
        "fn helper() {}",
        "// covers: C003",
        "let x = 1;",
        "// covers: C004",
    ]);
    let (tests, problems) = marked_tests("a.rs", &text);
    // `helper` is a fn, so the first marker attaches to it; the `let` line strands the second;
    // the marker at the end of the file has nothing after it.
    assert_eq!(tests, vec![("a.rs::helper".to_owned(), ids(&["C001"]))]);
    assert_eq!(
        problems,
        vec![
            "a.rs:1: `not-an-id` is not a contract id".to_owned(),
            "a.rs:3: covers marker is not followed by a test fn".to_owned(),
            "a.rs:5: covers marker is not followed by a test fn".to_owned(),
        ]
    );
}

#[test]
fn allowlisted_id_is_excluded_with_its_reason_not_uncovered() {
    let inventory = ids(&["C001", "C002"]);
    let exclusions = vec![("C002".to_owned(), "needs a browser".to_owned())];
    let c = coverage(&inventory, &[scenario("alpha", &["C001"])], &exclusions);
    assert_eq!(c.excluded["C002"], "needs a browser");
    assert!(c.uncovered().is_empty());
    let text = report_text(&c, &[]);
    assert!(text.starts_with(
        "coverage: 1 of 2 contract ids covered, 1 excluded with a reason, 0 uncovered\n"
    ));
    assert!(text.contains("excluded:\n  C002  needs a browser\n"));
    let value = report_json(&c, &[]);
    assert_eq!(value["covered"], 1);
    assert_eq!(value["excluded"]["C002"], "needs a browser");
    assert_eq!(value["uncovered"], serde_json::json!([]));
}

#[test]
fn covered_id_on_the_allowlist_is_not_reported_as_excluded() {
    let inventory = ids(&["C001"]);
    let exclusions = vec![("C001".to_owned(), "unused".to_owned())];
    let c = coverage(&inventory, &[scenario("alpha", &["C001"])], &exclusions);
    assert!(c.excluded.is_empty());
    assert_eq!(c.by_id["C001"], vec!["alpha"]);
}

#[test]
fn allowlisted_id_outside_the_inventory_is_flagged() {
    let inventory = ids(&["C001"]);
    let exclusions = vec![("Z9".to_owned(), "gone".to_owned())];
    let c = coverage(&inventory, &[], &exclusions);
    assert_eq!(c.unknown_exclusions, vec!["Z9"]);
    assert!(c.excluded.is_empty());
    assert!(report_text(&c, &[]).contains("allowlisted but not in the inventory:\n  Z9\n"));
}

#[test]
fn allowlist_parses_entries_and_rejects_incomplete_ones() {
    let good = "\
[[exclusion]]
id = \"C002\"
reason = \"ui screenshot needs a browser\"
";
    assert_eq!(
        parse_exclusions(good).unwrap(),
        vec![(
            "C002".to_owned(),
            "ui screenshot needs a browser".to_owned()
        )]
    );
    assert!(parse_exclusions("").unwrap().is_empty());
    let no_reason = "[[exclusion]]\nid = \"C002\"\n";
    assert!(
        parse_exclusions(no_reason)
            .unwrap_err()
            .contains("missing `reason`")
    );
    let blank_reason = "[[exclusion]]\nid = \"C002\"\nreason = \"  \"\n";
    assert!(
        parse_exclusions(blank_reason)
            .unwrap_err()
            .contains("missing `reason`")
    );
    let bad_id = "[[exclusion]]\nid = \"c2\"\nreason = \"x\"\n";
    assert!(
        parse_exclusions(bad_id)
            .unwrap_err()
            .contains("not a contract id")
    );
    let twice = format!("{good}{good}");
    assert!(
        parse_exclusions(&twice)
            .unwrap_err()
            .contains("listed twice")
    );
}

#[test]
fn repository_allowlist_parses_and_names_only_inventory_ids() {
    let root = repo_root();
    let markdown = std::fs::read_to_string(root.join("docs").join("rust-rewrite-contracts.md"))
        .expect("inventory");
    let inventory = inventory_ids(&markdown);
    let path = root
        .join("tests")
        .join("conformance")
        .join("coverage-allowlist.toml");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let exclusions = parse_exclusions(&text).expect("allowlist parses");
    for (id, reason) in &exclusions {
        assert!(inventory.contains(id), "{id} is not in the inventory");
        assert!(!reason.is_empty());
    }
}
