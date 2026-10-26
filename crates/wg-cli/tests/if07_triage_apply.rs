//! IF-07: `task triage-apply`, run through the real binary against a real SQLite database. A dry
//! run plans and changes nothing; a real run applies each verdict's steps; an element that cannot
//! be planned is reported and makes the command exit 1.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use wg_model::Write;
use wg_model::entity::Workspace;
use wg_runtime::App;
use wg_types::Timestamp;

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-if07-{name}-{}", std::process::id()));
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

/// Writes the triage file under the home's directory and returns its path.
fn triage_file(home: &Path, verdicts: &Value) -> PathBuf {
    let path = home.join("triage.json");
    std::fs::create_dir_all(home).expect("the home exists");
    std::fs::write(&path, verdicts.to_string()).expect("the triage file is written");
    path
}

// covers: C166, O05
#[test]
fn a_dry_run_reports_the_plan_and_changes_nothing() {
    let home = home_with_workspace("dry-run");
    json(&home, &["task", "create", "Stale work"], 0);
    json(&home, &["task", "create", "Other work"], 0);
    let file = triage_file(
        &home,
        &serde_json::json!([
            {"key": "AC-1", "verdict": "resolved", "summary": "done", "evidence": ["ok"]},
            {"key": "AC-9", "verdict": "unclear"}
        ]),
    );
    let file = file.to_str().expect("a UTF-8 path");
    let report = json(&home, &["task", "triage-apply", file, "--dry-run"], 1);
    assert_eq!(report["applied"][0]["key"], "AC-1");
    assert_eq!(report["applied"][0]["action"], "cancelled");
    assert_eq!(report["applied"][0]["new_keys"], serde_json::json!([]));
    assert_eq!(report["skipped"], serde_json::json!([]));
    assert_eq!(report["errors"][0]["key"], "AC-9");
    assert_eq!(report["errors"][0]["index"], 1);
    let shown = json(&home, &["task", "show", "AC-1"], 0);
    assert_eq!(shown["status"], "backlog");
    assert_eq!(shown["tags"], serde_json::json!([]));
}

// covers: C166, C149
#[test]
fn a_real_run_cancels_with_a_comment_and_a_triage_tag() {
    let home = home_with_workspace("resolve");
    json(&home, &["task", "create", "Stale work"], 0);
    let file = triage_file(
        &home,
        &serde_json::json!([
            {"key": "AC-1", "verdict": "obsolete", "summary": "gone", "evidence": ["x.rs"]}
        ]),
    );
    let file = file.to_str().expect("a UTF-8 path");
    let report = json(
        &home,
        &["task", "triage-apply", file, "--source", "triage batch 1"],
        0,
    );
    assert_eq!(report["applied"][0]["action"], "cancelled");
    assert_eq!(report["errors"], serde_json::json!([]));
    let shown = json(&home, &["task", "show", "AC-1"], 0);
    assert_eq!(shown["status"], "cancelled");
    assert_eq!(shown["tags"], serde_json::json!(["triage:obsolete"]));
    let comment = shown["comments"]
        .as_array()
        .and_then(|list| list.last())
        .expect("the verdict left a comment");
    assert_eq!(
        comment["body"],
        "Triage (triage batch 1): obsolete. gone\n\nEvidence:\n- x.rs"
    );
}

// covers: C166
#[test]
fn an_open_verdict_creates_its_follow_up_and_supersedes_the_original() {
    let home = home_with_workspace("open");
    json(
        &home,
        &["task", "create", "Rewrite me", "--priority", "high"],
        0,
    );
    let file = triage_file(
        &home,
        &serde_json::json!([
            {
                "key": "AC-1",
                "verdict": "open",
                "summary": "partly done",
                "rewrites": [{
                    "title": "Finish the rewrite",
                    "description": "Port the rest.",
                    "files": ["crates/wg-tasks/src/triage.rs"],
                    "acceptance_criteria": ["it works"],
                    "size": "one-file"
                }]
            }
        ]),
    );
    let file = file.to_str().expect("a UTF-8 path");
    let report = json(&home, &["task", "triage-apply", file], 0);
    assert_eq!(report["applied"][0]["action"], "superseded");
    assert_eq!(
        report["applied"][0]["new_keys"],
        serde_json::json!(["AC-2"])
    );
    let follow = json(&home, &["task", "show", "AC-2"], 0);
    assert_eq!(follow["title"], "Finish the rewrite");
    assert_eq!(follow["priority"], "high");
    assert_eq!(
        follow["tags"],
        serde_json::json!(["scope:one-file", "triaged"])
    );
    assert_eq!(follow["relations"][0]["relation_type"], "follows_up");
    let original = json(&home, &["task", "show", "AC-1"], 0);
    assert_eq!(original["status"], "cancelled");
    assert_eq!(original["tags"], serde_json::json!(["triage:superseded"]));
}
