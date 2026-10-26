//! The `decision list, show, request, choose, withdraw` commands, run through the real binary
//! against a real SQLite database. The workspace is written through the store, because `init`
//! belongs to another group. The expected output was checked against the Python CLI
//! (`tests/integration/test_decisions.py` scenarios, replayed on both implementations).

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use wg_model::Write;
use wg_model::entity::Workspace;
use wg_runtime::App;
use wg_types::Timestamp;

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-decisions-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A home with the workspace `acme` (prefix `AC`) and the task AC-1.
fn home_with_task(name: &str) -> PathBuf {
    let home = temp_home(name);
    {
        let mut app = App::open(Some(&home)).expect("the app opens");
        let at = Timestamp::parse_iso("2026-01-01T00:00:00Z").expect("a timestamp");
        let id = wg_model::derive_id(&[7; 32], 0);
        app.runtime
            .store
            .commit_decision(&wg_model::Decision::new(()).write(Write::insert(Workspace {
                id,
                slug: "acme".to_owned(),
                name: "Acme".to_owned(),
                description: String::new(),
                key_prefix: "AC".to_owned(),
                next_task_number: 1,
                settings: serde_json::Map::new(),
                archived_at: None,
                created_at: at,
                updated_at: at,
            })))
            .expect("the workspace is written");
    }
    json(&home, &["task", "create", "Pick a database"], 0);
    home
}

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

fn json(home: &Path, args: &[&str], expect: i32) -> Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let (code, stdout, stderr) = run(home, &full);
    assert_eq!(code, expect, "{args:?}: {stdout}{stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"))
}

fn tags(home: &Path) -> Vec<String> {
    let task = json(home, &["task", "show", "AC-1"], 0);
    serde_json::from_value(task["tags"].clone()).expect("tags")
}

// covers: C189, C188, C190
#[test]
fn request_tags_the_task_and_the_list_and_show_report_it() {
    let home = home_with_task("request");
    let asked = json(
        &home,
        &[
            "decision",
            "request",
            "AC-1",
            "-q",
            "Which db?",
            "-o",
            "SQLite::embedded",
            "-o",
            "Postgres",
            "--recommend",
            "1",
        ],
        0,
    );
    assert_eq!(asked["status"], "open");
    assert_eq!(asked["task_key"], "AC-1");
    assert_eq!(asked["options"][0]["id"], "a");
    assert_eq!(asked["options"][0]["detail"], "embedded");
    assert_eq!(asked["options"][0]["recommended"], true);
    assert_eq!(asked["options"][1]["recommended"], false);
    assert!(tags(&home).contains(&"needs-decision".to_owned()));

    let listed = json(&home, &["decision", "list"], 0);
    assert_eq!(listed[0]["task"]["key"], "AC-1");
    assert_eq!(
        listed[0]["task"]["tags"],
        serde_json::json!(["needs-decision"])
    );

    let id = asked["id"].as_str().expect("an id");
    let by_prefix = json(&home, &["decision", "show", &id[..8]], 0);
    assert_eq!(by_prefix["id"], id);
    let (code, human, _) = run(&home, &["decision", "list"]);
    assert_eq!(code, 0);
    assert!(
        human.contains(&id[..8]) && human.contains("Which db?"),
        "{human}"
    );
}

// covers: C187, C191
#[test]
fn choosing_the_last_open_decision_clears_the_waiting_tags() {
    let home = home_with_task("choose");
    let first = json(
        &home,
        &[
            "decision", "request", "AC-1", "-q", "One?", "-o", "x", "-o", "y",
        ],
        0,
    );
    let second = json(&home, &["decision", "request", "AC-1", "-q", "Two?"], 0);
    let first_id = first["id"].as_str().expect("an id");
    let second_id = second["id"].as_str().expect("an id");

    let chosen = json(
        &home,
        &[
            "decision",
            "choose",
            &first_id[..8],
            "-o",
            "b",
            "--note",
            "  because ",
        ],
        0,
    );
    assert_eq!(chosen["status"], "decided");
    assert_eq!(chosen["chosen_option_id"], "b");
    assert_eq!(chosen["note"], "because");
    assert!(
        tags(&home).contains(&"needs-decision".to_owned()),
        "one is still open"
    );

    json(
        &home,
        &["decision", "withdraw", second_id, "--reason", "moot"],
        0,
    );
    assert!(!tags(&home).contains(&"needs-decision".to_owned()));
    let task = json(&home, &["task", "show", "AC-1"], 0);
    let bodies: Vec<&str> = task["comments"]
        .as_array()
        .expect("comments")
        .iter()
        .filter_map(|c| c["body"].as_str())
        .collect();
    assert!(
        bodies
            .iter()
            .any(|b| b.contains("One? -> y") && b.contains("Note: because"))
    );
    assert!(
        bodies
            .iter()
            .any(|b| b.contains("Decision withdrawn: Two?") && b.contains("Reason: moot"))
    );

    // An answered decision may be answered again, with free text.
    let again = json(
        &home,
        &["decision", "choose", first_id, "--other", "neither"],
        0,
    );
    assert_eq!(again["other_text"], "neither");
    assert_eq!(again["version"], 3);
}

// covers: C187, C188, C189, C191
#[test]
fn bad_input_and_unknown_decisions_are_refused_with_the_python_exit_codes() {
    let home = home_with_task("errors");
    let asked = json(
        &home,
        &["decision", "request", "AC-1", "-q", "One?", "-o", "x"],
        0,
    );
    let id = asked["id"].as_str().expect("an id");

    let err = json(&home, &["decision", "choose", id, "-o", "zzz"], 2);
    assert_eq!(err["error"]["code"], "validation_failed");
    assert!(
        err["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("options are: a"))
    );
    assert_eq!(
        json(&home, &["decision", "choose", id], 2)["error"]["code"],
        "validation_failed"
    );
    assert_eq!(
        json(
            &home,
            &["decision", "choose", id, "-o", "a", "--other", "x"],
            2
        )["error"]["code"],
        "validation_failed"
    );
    assert_eq!(
        json(&home, &["decision", "show", "deadbeef"], 4)["error"]["code"],
        "cli_error"
    );
    assert_eq!(
        json(
            &home,
            &["decision", "show", "00000000-0000-0000-0000-000000000000"],
            4
        )["error"]["code"],
        "not_found"
    );
    json(
        &home,
        &["decision", "request", "AC-1", "-q", "Q", "--recommend", "1"],
        2,
    );
    json(&home, &["decision", "list", "--status", "bogus"], 2);
    json(&home, &["decision", "request", "AC-9", "-q", "Q"], 4);
    json(&home, &["decision", "request", "AC-1", "-q", "  "], 2);

    json(&home, &["decision", "withdraw", id], 0);
    json(&home, &["decision", "withdraw", id], 2);
    json(&home, &["decision", "choose", id, "-o", "a"], 2);
}
