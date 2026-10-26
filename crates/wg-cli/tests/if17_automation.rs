//! The `automation` commands, run through the real binary against a real SQLite database. The
//! shapes and exit codes follow the Python CLI (`src/workgraph/cli/commands/automation.py`), which
//! was checked against this binary on temporary homes with the same command sequence.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-automation-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Runs the binary with `--home` and returns the exit code, stdout and stderr.
fn run(home: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_workgraph"))
        .arg("--home")
        .arg(home)
        .args(args)
        .env("COLUMNS", "120")
        .env("NO_COLOR", "1")
        .output()
        .expect("the binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Runs a command with `--json` and parses its standard output.
fn json(home: &Path, args: &[&str]) -> (i32, Value) {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let (code, stdout, _) = run(home, &full);
    let value = serde_json::from_str(stdout.trim()).expect("the output is JSON");
    (code, value)
}

/// A fresh home with the default workspace.
fn initialised(name: &str) -> PathBuf {
    let home = temp_home(name);
    let (code, _, stderr) = run(&home, &["init"]);
    assert_eq!(code, 0, "init failed: {stderr}");
    home
}

const EVENT_RULE: &str = r#"{"name":"tag-high","trigger":{"type":"event","event_types":["task.created"]},"actions":[{"type":"add_tags","tags":["triaged"]}],"conditions":{"field":"task.priority","op":"eq","value":"high"}}"#;

// covers: C025
#[test]
fn catalog_lists_triggers_schedules_actions_and_operators() {
    let home = initialised("catalog");
    let (code, value) = json(&home, &["automation", "catalog"]);
    assert_eq!(code, 0);
    assert_eq!(value["triggers"]["task.created"], "Task created");
    assert_eq!(value["manual"], Value::Bool(true));
    assert_eq!(
        value["schedule"],
        serde_json::json!(["cron", "interval_seconds"])
    );
    assert!(
        value["operators"]
            .as_array()
            .is_some_and(|ops| ops.len() == 14)
    );
}

// covers: C026, C034
#[test]
fn create_stores_the_normalised_rule_and_show_reads_it_back() {
    let home = initialised("create");
    let (code, created) = json(&home, &["automation", "create", EVENT_RULE]);
    assert_eq!(code, 0);
    assert_eq!(created["name"], "tag-high");
    assert_eq!(created["version"], 1);
    // The shorthand predicate is stored as `{"predicate": ...}`, as Python stores it.
    assert_eq!(created["conditions"]["predicate"]["op"], "eq");
    let (code, shown) = json(&home, &["automation", "show", "tag-high"]);
    assert_eq!(code, 0);
    assert_eq!(shown["id"], created["id"]);
    assert_eq!(shown["trigger"]["type"], "event");
}

// covers: C026
#[test]
fn create_refuses_an_invalid_document_with_pydantic_errors() {
    let home = initialised("invalid");
    let (code, value) = json(
        &home,
        &[
            "automation",
            "create",
            r#"{"name":"bad","trigger":{"type":"event"},"actions":[]}"#,
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(value["error"]["code"], "validation_failed");
    assert_eq!(value["error"]["message"], "invalid automation rule");
    let errors = value["error"]["details"]["errors"]
        .as_array()
        .expect("the errors are listed");
    assert!(
        errors
            .iter()
            .any(|error| error["loc"] == serde_json::json!(["actions"]))
    );
}

// covers: C026
#[test]
fn create_refuses_a_duplicate_name_with_a_conflict() {
    let home = initialised("duplicate");
    let (code, _) = json(&home, &["automation", "create", EVENT_RULE]);
    assert_eq!(code, 0);
    let (code, value) = json(&home, &["automation", "create", EVENT_RULE]);
    assert_eq!(code, 3);
    assert_eq!(value["error"]["code"], "conflict");
}

// covers: C029
#[test]
fn list_shows_every_rule_by_name() {
    let home = initialised("list");
    let (code, _) = json(&home, &["automation", "create", EVENT_RULE]);
    assert_eq!(code, 0);
    let (code, value) = json(&home, &["automation", "list"]);
    assert_eq!(code, 0);
    let names: Vec<&str> = value
        .as_array()
        .expect("a list of rules")
        .iter()
        .filter_map(|rule| rule["name"].as_str())
        .collect();
    assert_eq!(names, ["tag-high"]);
}

// covers: C027, C028
#[test]
fn disable_and_enable_flip_the_rule_flag() {
    let home = initialised("toggle");
    let (code, _) = json(&home, &["automation", "create", EVENT_RULE]);
    assert_eq!(code, 0);
    let (code, value) = json(&home, &["automation", "disable", "tag-high"]);
    assert_eq!(code, 0);
    assert_eq!(value["enabled"], Value::Bool(false));
    let (code, value) = json(&home, &["automation", "enable", "tag-high"]);
    assert_eq!(code, 0);
    assert_eq!(value["enabled"], Value::Bool(true));
}

// covers: C036
#[test]
fn update_replaces_the_specification_and_bumps_the_version() {
    let home = initialised("update");
    let (code, _) = json(&home, &["automation", "create", EVENT_RULE]);
    assert_eq!(code, 0);
    let spec = r#"{"name":"tag-high","description":"renamed text","trigger":{"type":"manual"},"actions":[{"type":"notify","title":"hi"}]}"#;
    let (code, value) = json(&home, &["automation", "update", "tag-high", spec]);
    assert_eq!(code, 0);
    assert_eq!(value["version"], 2);
    assert_eq!(value["description"], "renamed text");
    assert_eq!(value["trigger"]["type"], "manual");
}

// covers: C035, C033
#[test]
fn manual_trigger_records_a_queued_run_that_runs_lists() {
    let home = initialised("trigger");
    let manual =
        r#"{"name":"hand","trigger":{"type":"manual"},"actions":[{"type":"notify","title":"hi"}]}"#;
    let (code, _) = json(&home, &["automation", "create", manual]);
    assert_eq!(code, 0);
    let (code, run) = json(
        &home,
        &["automation", "trigger", "hand", "--input", r#"{"n": 2}"#],
    );
    assert_eq!(code, 0);
    assert_eq!(run["status"], "queued");
    assert_eq!(run["trigger_type"], "manual");
    let (code, runs) = json(&home, &["automation", "runs", "--rule", "hand"]);
    assert_eq!(code, 0);
    assert_eq!(runs.as_array().map(Vec::len), Some(1));
}

// covers: C031
#[test]
fn preview_of_an_unsaved_document_reports_the_trace_and_records_nothing_queued() {
    let home = initialised("preview");
    let spec = r#"{"name":"p","trigger":{"type":"manual"},"actions":[{"type":"classify_task"}],"conditions":{"field":"input.n","op":"gt","value":10}}"#;
    let (code, run) = json(&home, &["automation", "preview", "--spec", spec]);
    assert_eq!(code, 0);
    assert_eq!(run["status"], "not_matched");
    assert_eq!(run["matched"], Value::Bool(false));
    assert_eq!(run["condition_trace"][0]["op"], "gt");
    assert_eq!(run["dry_run"], Value::Bool(true));
}

// covers: C032
#[test]
fn process_counts_the_init_event_once_and_then_nothing() {
    // `init` records the workspace event, which the first pass reads; a second pass finds none.
    let home = initialised("process");
    let (code, value) = json(&home, &["automation", "process"]);
    assert_eq!(code, 0);
    assert_eq!(value["events"]["events"], 1);
    assert_eq!(value["events"]["runs"], 0);
    assert_eq!(value["scheduled"], 0);
    let (code, value) = json(&home, &["automation", "process"]);
    assert_eq!(code, 0);
    assert_eq!(value["events"]["events"], 0);
}

// covers: C030
#[test]
fn notifications_is_empty_on_a_fresh_workspace() {
    let home = initialised("notes");
    let (code, value) = json(&home, &["automation", "notifications", "--unread"]);
    assert_eq!(code, 0);
    assert_eq!(value, serde_json::json!([]));
}
