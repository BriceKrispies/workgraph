//! Port of `tests/unit/test_contract.py`, `test_briefs.py` and `test_escape.py`: the same cases
//! over the Rust decisions, with the same expected text.

use serde_json::{Value, json};
use wg_experiments::briefs::{BriefContext, BriefError, render};
use wg_experiments::contract::contract_problems;
use wg_experiments::escape::{ToolCall, find_escapes, normalize_path_text};

const ROOT: &str = r"C:\dev\rho";

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

/// The `_contract()` helper of the Python test: a complete contract, overridable field by field.
fn contract(overrides: &[(&str, Value)]) -> Value {
    let mut fields = serde_json::Map::new();
    fields.insert("files".into(), json!(["pkg/mod.py", "tests/test_mod.py"]));
    fields.insert(
        "oracle".into(),
        json!([["pytest", "-q", "tests/test_mod.py"]]),
    );
    fields.insert("gates".into(), json!([["ruff", "check", "."]]));
    fields.insert("out_of_scope".into(), json!(["the CLI"]));
    fields.insert("checks".into(), json!(["tests/test_mod.py::test_f"]));
    fields.insert(
        "predicted".into(),
        json!({"src_lines": 40, "src_files": 1, "packages": ["pkg"]}),
    );
    for (key, value) in overrides {
        fields.insert((*key).to_owned(), value.clone());
    }
    Value::Object(fields)
}

fn metadata(contract: Option<Value>) -> Value {
    match contract {
        Some(contract) => json!({"contract": contract}),
        None => json!({}),
    }
}

/// `contract_problems` of a task with the given tags, criteria and contract.
fn problems(
    tags: &[&str],
    criteria: &[&str],
    contract: Option<Value>,
) -> (Vec<String>, Vec<String>) {
    let found = contract_problems(&strings(tags), &strings(criteria), &metadata(contract));
    (found.missing, found.not_delegable)
}

const ONE_CRITERION: &[&str] = &["the function returns 1"];

// ---- test_contract.py

#[test]
fn test_complete_contract_has_no_problems() {
    assert_eq!(
        problems(&[], ONE_CRITERION, Some(contract(&[]))),
        (vec![], vec![])
    );
}

#[test]
fn test_each_not_delegable_tag_is_named() {
    for tag in [
        "needs-decision",
        "kind:report",
        "kind:register-row",
        "kind:split-status",
        "kind:manifest",
    ] {
        let (missing, not_delegable) = problems(&[tag], ONE_CRITERION, Some(contract(&[])));
        assert!(missing.is_empty(), "{tag}");
        assert_eq!(not_delegable.len(), 1, "{tag}");
        assert!(not_delegable[0].contains(tag), "{tag}");
    }
}

#[test]
fn test_unrelated_tags_are_delegable() {
    assert_eq!(
        problems(&["capability-frontier"], ONE_CRITERION, Some(contract(&[]))),
        (vec![], vec![])
    );
}

#[test]
fn test_no_oracle_and_no_criteria_is_not_delegable() {
    let (missing, not_delegable) = problems(
        &[],
        &[],
        Some(contract(&[("oracle", json!([])), ("checks", json!([]))])),
    );
    assert!(not_delegable[0].contains("no checkable oracle"));
    assert!(missing.iter().any(|m| m == "contract.oracle"));
}

#[test]
fn test_criteria_without_a_contract_oracle_are_still_checkable() {
    let (missing, not_delegable) =
        problems(&[], ONE_CRITERION, Some(contract(&[("oracle", json!([]))])));
    assert!(not_delegable.is_empty());
    assert_eq!(missing, strings(&["contract.oracle"]));
}

#[test]
fn test_missing_contract_names_every_field() {
    let (missing, not_delegable) = problems(&[], ONE_CRITERION, None);
    assert!(not_delegable.is_empty());
    assert_eq!(
        missing[..5],
        strings(&[
            "contract.files",
            "contract.oracle",
            "contract.gates",
            "contract.out_of_scope",
            "contract.checks (1 acceptance criteria, none checks, one non-empty each)",
        ])[..]
    );
    assert_eq!(
        missing[5..],
        strings(&[
            "contract.predicted.src_lines",
            "contract.predicted.src_files",
            "contract.predicted.packages",
        ])[..]
    );
}

#[test]
fn test_malformed_fields_are_named() {
    let malformed = contract(&[
        ("files", json!([])),
        ("oracle", json!([["pytest", ""]])),
        ("gates", json!([[]])),
        ("out_of_scope", json!([""])),
        (
            "predicted",
            json!({"src_lines": 0, "src_files": true, "packages": "pkg"}),
        ),
    ]);
    let (missing, _) = problems(&[], ONE_CRITERION, Some(malformed));
    assert_eq!(
        missing,
        strings(&[
            "contract.files",
            "contract.oracle",
            "contract.gates",
            "contract.out_of_scope",
            "contract.predicted.src_lines",
            "contract.predicted.src_files",
            "contract.predicted.packages",
        ])
    );
}

#[test]
fn test_gates_may_be_empty() {
    assert_eq!(
        problems(&[], ONE_CRITERION, Some(contract(&[("gates", json!([]))]))),
        (vec![], vec![])
    );
}

#[test]
fn test_packages_may_be_empty() {
    let predicted = json!({"src_lines": 4, "src_files": 1, "packages": []});
    assert_eq!(
        problems(
            &[],
            ONE_CRITERION,
            Some(contract(&[("predicted", predicted)]))
        ),
        (vec![], vec![])
    );
}

#[test]
fn test_checks_must_match_the_criteria_count() {
    let (missing, _) = problems(
        &[],
        &["one", "two"],
        Some(contract(&[("checks", json!(["tests/a.py::t"]))])),
    );
    assert_eq!(
        missing,
        strings(&["contract.checks (2 acceptance criteria, 1 checks, one non-empty each)"])
    );
}

#[test]
fn test_blank_check_is_malformed() {
    let (missing, _) = problems(&[], &["one"], Some(contract(&[("checks", json!(["  "]))])));
    assert_eq!(
        missing,
        strings(&["contract.checks (1 acceptance criteria, 1 checks, one non-empty each)"])
    );
}

// ---- test_briefs.py

fn base_context() -> BriefContext {
    BriefContext {
        task_key: "WG-1".to_owned(),
        trial_id: "T-1".to_owned(),
        repository: "demo".to_owned(),
        worktree: "/w/demo-wt".to_owned(),
        branch: "agent/wg-1".to_owned(),
        subject: "Add g".to_owned(),
        body: "Return g.".to_owned(),
        test_paths: strings(&["tests/test_mod.py"]),
        oracle: vec![strings(&[
            "python",
            "-m",
            "pytest",
            "-q",
            "tests/test_mod.py",
        ])],
        gates: vec![strings(&["uv", "run", "ruff", "check", "."])],
        acceptance: Vec::new(),
        files: Vec::new(),
        out_of_scope: Vec::new(),
    }
}

fn leaf_context() -> BriefContext {
    BriefContext {
        acceptance: vec![(
            "g returns two".to_owned(),
            "tests/test_mod.py::test_g".to_owned(),
        )],
        files: strings(&["pkg/mod.py", "tests/test_mod.py"]),
        out_of_scope: strings(&["the CLI"]),
        ..base_context()
    }
}

const GOLDEN_REPLAY_V1: &str = "Task-ID: WG-1
Trial-ID: T-1

You are working alone in the git worktree /w/demo-wt (repository demo), on branch agent/wg-1. Work only there. Do not push or merge.

## Task
Add g

Return g.

## Tests that must pass
These test files were added or changed for this task and fail now:
- tests/test_mod.py

Make them pass by changing the code, not the tests. Do not modify, delete or skip these test files.

Run them with:
    python -m pytest -q tests/test_mod.py

## Checks that must also pass
    uv run ruff check .

## Rules
Follow the repository's own agent instructions (AGENTS.md or CLAUDE.md) if it has them. Commit your work on agent/wg-1 when you are done.

## Report
Reply with what you changed, the output of the test command, and anything you could not finish.
";

const GOLDEN_LEAF_V1: &str = "Task-ID: WG-1
Trial-ID: T-1

You are working alone in the git worktree /w/demo-wt (repository demo), on branch agent/wg-1. Work only there. Do not push or merge.

## Task
Add g

Return g.

## Done means
- g returns two (checked by: tests/test_mod.py::test_g)

## Files you may change
- pkg/mod.py
- tests/test_mod.py

## Out of scope
- the CLI

## Tests that must pass
These tests were written for this task and fail now. Make them pass by changing the code, not the tests. Do not modify, delete or skip them.
    python -m pytest -q tests/test_mod.py

## Checks that must also pass
    uv run ruff check .

## Rules
Follow the repository's own agent instructions (AGENTS.md or CLAUDE.md) if it has them. Change only the files listed above. Commit your work on agent/wg-1 when you are done.

## Report
Reply with what you changed, the output of the test command, and anything you could not finish.
";

fn rendered(template: &str, context: &BriefContext) -> String {
    render(template, context).unwrap_or_default()
}

#[test]
fn test_replay_v1_matches_golden_text() {
    assert_eq!(rendered("replay-v1", &base_context()), GOLDEN_REPLAY_V1);
}

#[test]
fn test_no_gates_renders_none() {
    let text = rendered(
        "replay-v1",
        &BriefContext {
            gates: Vec::new(),
            ..base_context()
        },
    );
    assert!(text.contains("## Checks that must also pass\n    (none)\n\n## Rules\n"));
    assert!(!text.contains("ruff"));
}

#[test]
fn test_empty_body_is_followed_directly_by_next_section() {
    let text = rendered(
        "replay-v1",
        &BriefContext {
            body: String::new(),
            ..base_context()
        },
    );
    assert!(text.contains("## Task\nAdd g\n\n## Tests that must pass\n"));
    assert!(!text.contains("\n\n\n"));
}

#[test]
fn test_unknown_template_is_rejected_naming_known_ids() {
    let error = render("replay-v9", &base_context());
    assert_eq!(
        error,
        Err(BriefError::UnknownTemplate {
            template: "replay-v9".to_owned()
        })
    );
    let message = error.err().map(|e| e.message()).unwrap_or_default();
    assert!(message.contains("replay-v1"), "{message}");
    assert!(message.contains("replay-v9"), "{message}");
}

#[test]
fn test_rendered_output_ends_with_exactly_one_newline() {
    let text = rendered("replay-v1", &base_context());
    assert!(text.ends_with("anything you could not finish.\n"));
    assert!(!text.ends_with("\n\n"));
}

#[test]
fn test_leaf_v1_matches_golden_text() {
    assert_eq!(rendered("leaf-v1", &leaf_context()), GOLDEN_LEAF_V1);
}

#[test]
fn test_leaf_v1_empty_body_and_no_gates() {
    let text = rendered(
        "leaf-v1",
        &BriefContext {
            body: String::new(),
            gates: Vec::new(),
            ..leaf_context()
        },
    );
    assert!(text.contains("## Task\nAdd g\n\n## Done means\n"));
    assert!(text.contains("## Checks that must also pass\n    (none)\n\n## Rules\n"));
    assert!(!text.contains("\n\n\n"));
    assert!(text.ends_with("anything you could not finish.\n"));
}

#[test]
fn test_leaf_v1_lists_none_when_a_section_is_empty() {
    let text = rendered(
        "leaf-v1",
        &BriefContext {
            acceptance: Vec::new(),
            out_of_scope: Vec::new(),
            ..leaf_context()
        },
    );
    assert!(text.contains("## Done means\n- (none)\n"));
    assert!(text.contains("## Out of scope\n- (none)\n"));
}

#[test]
fn test_replay_v1_ignores_leaf_only_fields() {
    let text = rendered(
        "replay-v1",
        &BriefContext {
            acceptance: vec![(
                "g returns two".to_owned(),
                "tests/test_mod.py::test_g".to_owned(),
            )],
            files: strings(&["pkg/mod.py"]),
            out_of_scope: strings(&["the CLI"]),
            ..base_context()
        },
    );
    assert_eq!(text, GOLDEN_REPLAY_V1);
}

// ---- test_escape.py

fn call(tool_input: Value, name: &str, tool_use_id: &str) -> ToolCall {
    ToolCall {
        name: name.to_owned(),
        input: tool_input,
        tool_use_id: tool_use_id.to_owned(),
    }
}

fn read(tool_input: Value) -> ToolCall {
    call(tool_input, "Read", "t1")
}

fn roots(items: &[&str]) -> Vec<String> {
    strings(items)
}

#[test]
fn test_backslashes_become_slashes_and_text_is_lowercased() {
    assert_eq!(normalize_path_text(r"C:\Dev\Rho\src"), "c:/dev/rho/src");
}

#[test]
fn test_msys_drive_prefix_becomes_a_windows_drive() {
    assert_eq!(
        normalize_path_text("cd /c/dev/rho && git log"),
        "cd c:/dev/rho && git log"
    );
    assert_eq!(normalize_path_text("/D/work"), "d:/work");
}

#[test]
fn test_repeated_slashes_collapse_before_the_drive_prefix_is_read() {
    assert_eq!(normalize_path_text("c://dev///rho"), "c:/dev/rho");
    assert_eq!(normalize_path_text("//c/dev"), "c:/dev");
}

#[test]
fn test_relative_segments_that_look_like_drives_are_left_alone() {
    assert_eq!(normalize_path_text("c:/c/dev"), "c:/c/dev");
    assert_eq!(normalize_path_text("src/a/b"), "src/a/b");
}

#[test]
fn test_a_sibling_directory_with_the_root_as_prefix_does_not_match() {
    assert!(
        find_escapes(
            &[read(json!({"file_path": "c:/dev/rho-other/x.rs"}))],
            &roots(&[ROOT])
        )
        .is_empty()
    );
}

#[test]
fn test_the_root_itself_and_paths_under_it_match() {
    for value in [
        "c:/dev/rho",
        "c:/dev/rho/x",
        "\"c:/dev/rho\"",
        "cd /c/dev/rho && git log",
    ] {
        let escapes = find_escapes(&[read(json!({"command": value}))], &roots(&[ROOT]));
        assert_eq!(escapes.len(), 1, "{value}");
        assert_eq!(escapes[0].root, ROOT, "{value}");
        assert_eq!(escapes[0].excerpt, value, "{value}");
    }
}

#[test]
fn test_root_followed_by_shell_operators_matches() {
    for value in [
        "c:/dev/rho;ls",
        "c:/dev/rho&&ls",
        "c:/dev/rho|cat",
        "c:/dev/rho\nls",
    ] {
        assert_eq!(
            find_escapes(&[read(json!({"command": value}))], &roots(&[ROOT])).len(),
            1,
            "{value}"
        );
    }
}

#[test]
fn test_nested_dict_and_list_values_are_searched() {
    let tool_input = json!({
        "edits": [{"old": "a", "new": "b"}, {"path": ["x", {"deep": r"C:\dev\rho\pkg\mod.py"}]}],
        "meta": {"description": "unrelated"},
    });
    let escapes = find_escapes(&[call(tool_input, "Edit", "t1")], &roots(&[ROOT]));
    let summary: Vec<(&str, &str)> = escapes
        .iter()
        .map(|e| (e.tool.as_str(), e.root.as_str()))
        .collect();
    assert_eq!(summary, vec![("Edit", ROOT)]);
    assert_eq!(escapes[0].excerpt, r"C:\dev\rho\pkg\mod.py");
}

#[test]
fn test_keys_and_non_string_values_are_ignored() {
    assert!(
        find_escapes(
            &[read(json!({"c:/dev/rho": 1, "n": 2, "flag": true}))],
            &roots(&[ROOT])
        )
        .is_empty()
    );
}

#[test]
fn test_one_escape_per_call_and_root() {
    let tool_input = json!({"a": "c:/dev/rho/x", "b": "c:/dev/rho/y", "c": "C:/dev/rho/other"});
    let escapes = find_escapes(
        &[read(tool_input.clone())],
        &roots(&[ROOT, "c:/dev/rho-other-clone/../rho"]),
    );
    let found: Vec<&str> = escapes.iter().map(|e| e.root.as_str()).collect();
    assert_eq!(found, vec![ROOT]);

    let nested = find_escapes(
        &[call(tool_input, "Read", "t9")],
        &roots(&[r"C:\dev\rho", r"C:\dev\rho\other"]),
    );
    let mut pairs: Vec<(&str, &str)> = nested
        .iter()
        .map(|e| (e.tool_use_id.as_str(), e.root.as_str()))
        .collect();
    pairs.sort_unstable();
    assert_eq!(
        pairs,
        vec![("t9", r"C:\dev\rho"), ("t9", r"C:\dev\rho\other")]
    );
}

#[test]
fn test_each_call_is_checked_separately() {
    let calls = [
        call(json!({"f": "c:/dev/rho/a"}), "Read", "t1"),
        call(json!({"f": "c:/other/a"}), "Read", "t2"),
    ];
    let escapes = find_escapes(&calls, &roots(&[ROOT]));
    let ids: Vec<&str> = escapes.iter().map(|e| e.tool_use_id.as_str()).collect();
    assert_eq!(ids, vec!["t1"]);
}

#[test]
fn test_excerpt_is_cut_to_300_characters() {
    let long_value = format!("c:/dev/rho/{}", "x".repeat(500));
    let escapes = find_escapes(&[read(json!({"command": long_value}))], &roots(&[ROOT]));
    assert_eq!(escapes[0].excerpt.chars().count(), 300);
}

#[test]
fn test_empty_roots_are_ignored() {
    assert!(find_escapes(&[read(json!({"command": "anything"}))], &roots(&["", "/"])).is_empty());
}
