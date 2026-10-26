//! IF-05: the `task create, list, show, update, bulk-update, archive, history, graph, ready,
//! promote-ready` commands, run through the real binary against a real SQLite database. The
//! workspace is written through the store, because `init` belongs to another group.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use wg_model::Write;
use wg_model::entity::Workspace;
use wg_runtime::App;
use wg_types::Timestamp;

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-if05-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A home with the workspace `acme` (prefix `AC`).
fn home_with_workspace(name: &str) -> PathBuf {
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
    home
}

fn run(home: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_workgraph"))
        .arg("--home")
        .arg(home)
        .arg("--json")
        .args(args)
        .env("COLUMNS", "120")
        .output()
        .expect("the binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn json(home: &Path, args: &[&str], expect: i32) -> Value {
    let (code, stdout, stderr) = run(home, args);
    assert_eq!(code, expect, "{args:?}: {stdout}{stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"))
}

// covers: C152, C165, C157, C167
#[test]
fn create_show_list_and_update_a_task() {
    let home = home_with_workspace("crud");
    let task = json(
        &home,
        &[
            "task",
            "create",
            "Retro it",
            "-t",
            "rho,retro",
            "-t",
            " bug , ,",
        ],
        0,
    );
    assert_eq!(task["key"], "AC-1");
    assert_eq!(task["status"], "backlog");
    let shown = json(&home, &["task", "show", "AC-1"], 0);
    assert_eq!(shown["tags"], serde_json::json!(["bug", "retro", "rho"]));
    let updated = json(
        &home,
        &[
            "task",
            "update",
            "AC-1",
            "--add-tag",
            "ops,ci",
            "--remove-tag",
            "bug, ",
        ],
        0,
    );
    let mut tags: Vec<String> = serde_json::from_value(updated["tags"].clone()).expect("tags");
    tags.sort();
    assert_eq!(tags, ["ci", "ops", "retro", "rho"]);
    let listing = json(&home, &["task", "list", "--tag", "ops"], 0);
    assert_eq!(listing["total"], 1);
    let found = json(&home, &["task", "list", "--search", "Retro"], 0);
    assert_eq!(found["items"][0]["key"], "AC-1");
}

// covers: C165
#[test]
fn show_several_refs_reports_the_failures_and_exits_with_the_first() {
    let home = home_with_workspace("show");
    json(&home, &["task", "create", "Known"], 0);
    let out = json(&home, &["task", "show", "AC-404", "AC-1"], 4);
    assert_eq!(out["items"][0]["key"], "AC-1");
    assert_eq!(out["errors"][0]["ref"], "AC-404");
    assert_eq!(out["errors"][0]["error"]["code"], "not_found");
}

// covers: C157
#[test]
fn list_limit_above_the_maximum_is_a_validation_error() {
    let home = home_with_workspace("limit");
    let err = json(&home, &["task", "list", "--limit", "2000"], 2);
    assert_eq!(err["error"]["code"], "validation_failed");
    assert!(
        err["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("500"))
    );
}

// covers: C159
#[test]
fn promote_ready_needs_a_scope_and_counts_per_project() {
    let home = home_with_workspace("promote");
    json(&home, &["task", "create", "Plain"], 0);
    for args in [
        vec!["task", "promote-ready"],
        vec!["task", "promote-ready", "--all", "--tag", "x"],
    ] {
        let err = json(&home, &args, 2);
        assert_eq!(err["error"]["code"], "validation_failed");
    }
    let dry = json(&home, &["task", "promote-ready", "--all", "--dry-run"], 0);
    assert_eq!(dry["promoted"], serde_json::json!(["AC-1"]));
    assert_eq!(dry["dry_run"], true);
    assert_eq!(
        json(&home, &["task", "show", "AC-1"], 0)["status"],
        "backlog"
    );
    json(&home, &["task", "promote-ready", "--all"], 0);
    assert_eq!(json(&home, &["task", "show", "AC-1"], 0)["status"], "ready");
}

// covers: C152, C160, C154, C155
#[test]
fn dependencies_block_ready_and_show_in_graph_and_history() {
    let home = home_with_workspace("graph");
    json(&home, &["task", "create", "Base", "--ready"], 0);
    let child = json(
        &home,
        &[
            "task",
            "create",
            "Child",
            "--parent",
            "AC-1",
            "--depends-on",
            "AC-1",
            "--ready",
        ],
        0,
    );
    assert_eq!(child["status"], "blocked");
    let ready = json(&home, &["task", "ready"], 0);
    assert_eq!(ready.as_array().map(Vec::len), Some(1));
    let graph = json(&home, &["task", "graph"], 0);
    assert_eq!(graph["edges"].as_array().map(Vec::len), Some(2));
    let history = json(&home, &["task", "history", "AC-2"], 0);
    assert_eq!(history[0]["event_type"], "task.created");
    assert_eq!(history[1]["event_type"], "task.status_changed");
}

// covers: C146, C147, C167
#[test]
fn archive_and_bulk_update_change_state_and_report_failures() {
    let home = home_with_workspace("bulk");
    json(&home, &["task", "create", "A"], 0);
    let archived = json(&home, &["task", "archive", "AC-1"], 0);
    assert!(archived["archived_at"].is_string());
    assert_eq!(json(&home, &["task", "list"], 0)["total"], 0);
    json(&home, &["task", "archive", "AC-1", "--undo"], 0);
    let bulk = json(
        &home,
        &["task", "bulk-update", "AC-1", "AC-9", "--priority", "low"],
        3,
    );
    assert_eq!(bulk["updated"], serde_json::json!(["AC-1"]));
    assert!(bulk["failed"]["AC-9"].is_string());
    let refused = json(
        &home,
        &["task", "update", "AC-1", "--status", "accepted"],
        3,
    );
    assert_eq!(refused["error"]["code"], "invalid_transition");
}
