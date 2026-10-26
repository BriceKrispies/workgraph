//! IF-06: the `task claim, release, complete, progress, result` commands, run through the real
//! binary against a real SQLite database. The workspace is written through the store, because
//! `init` belongs to another group.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use wg_model::Write;
use wg_model::entity::Workspace;
use wg_runtime::App;
use wg_types::Timestamp;

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-if06-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A home with the workspace `acme` (prefix `AC`).
fn home_with_workspace(name: &str) -> PathBuf {
    let home = temp_home(name);
    {
        let mut app = App::open(Some(&home)).expect("the app opens");
        let at = Timestamp::parse_iso("2026-01-01T00:00:00Z").expect("a timestamp");
        let id = wg_model::derive_id(&[6; 32], 0);
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

// covers: C148, C163
#[test]
fn claim_and_release_move_the_task_and_its_claim() {
    let home = home_with_workspace("claim-release");
    json(&home, &["task", "create", "Fix", "--ready"], 0);
    let claimed = json(&home, &["task", "claim", "AC-1", "--agent", "bot"], 0);
    assert_eq!(claimed["status"], "claimed");
    assert_eq!(claimed["claimed_by"], "agent:bot");
    let released = json(&home, &["task", "release", "AC-1", "--reason", "bye"], 0);
    assert_eq!(released["status"], "ready");
    assert!(released["claimed_by"].is_null());
    // Releasing a task that holds no claim is refused by the App, not by the CLI.
    let (code, _, _) = run(&home, &["task", "release", "AC-1"]);
    assert_ne!(code, 0, "a released task has no claim to release");
}

// covers: C150, C158
#[test]
fn progress_and_complete_report_on_the_claimed_task() {
    let home = home_with_workspace("reports");
    json(&home, &["task", "create", "Fix", "--ready"], 0);
    json(&home, &["task", "claim", "AC-1"], 0);
    let progress = json(
        &home,
        &["task", "progress", "AC-1", "half done", "--percent", "50"],
        0,
    );
    assert_eq!(progress["body"], "half done");
    let completed = json(&home, &["task", "complete", "AC-1", "-m", "fixed"], 0);
    assert_eq!(completed["status"], "awaiting_review");
}

// covers: C164
#[test]
fn result_checks_the_outcome_before_it_reports() {
    let home = home_with_workspace("result");
    json(&home, &["task", "create", "Fix", "--ready"], 0);
    json(&home, &["task", "claim", "AC-1"], 0);
    // The CLI refuses an outcome other than succeeded or failed with exit 2, before the App runs.
    let (code, _, stderr) = run(&home, &["task", "result", "AC-1", "maybe"]);
    assert_eq!(code, 2, "{stderr}");
    let reported = json(&home, &["task", "result", "AC-2", "failed"], 4);
    assert!(reported.is_object(), "an unknown task is a domain error");
    json(
        &home,
        &["task", "result", "AC-1", "failed", "--summary", "nope"],
        0,
    );
}
