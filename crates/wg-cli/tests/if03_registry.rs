//! IF-03: the `workspace` and `project` groups, run through the real binary against a real home
//! directory.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-if03-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn run_args(home: &Path, args: &[&str]) -> (i32, String, String) {
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

fn json_at(home: &Path, args: &[&str], expect: i32) -> Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let (code, stdout, stderr) = run_args(home, &full);
    assert_eq!(code, expect, "{args:?}: {stdout}{stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"))
}

fn home_with_workspace(name: &str) -> PathBuf {
    let home = temp_home(name);
    json_at(&home, &["init", "-w", "acme", "--prefix", "AC"], 0);
    home
}

// covers: C181, C182, C184, C185, C180
#[test]
fn a_workspace_is_created_listed_shown_used_and_archived() {
    let home = home_with_workspace("ws");
    let beta = json_at(&home, &["workspace", "create", "beta", "--prefix", "BT"], 0);
    assert_eq!(beta["name"], "Beta");
    assert_eq!(beta["key_prefix"], "BT");
    assert!(
        home.join("config")
            .join("workspaces")
            .join("beta.toml")
            .is_file()
    );
    // The first workspace stays the default: creating another does not move it.
    let shown = json_at(&home, &["workspace", "show"], 0);
    assert_eq!(shown["workspace"]["slug"], "acme");
    assert_eq!(shown["projects"], 0);
    let clash = json_at(&home, &["workspace", "create", "beta"], 3);
    assert_eq!(clash["error"]["code"], "conflict");
    let used = json_at(&home, &["workspace", "use", "beta"], 0);
    assert_eq!(used["slug"], "beta");
    let default = json_at(&home, &["workspace", "show"], 0);
    assert_eq!(default["workspace"]["slug"], "beta");
    json_at(&home, &["workspace", "archive", "beta"], 0);
    let live = json_at(&home, &["workspace", "list"], 0);
    assert_eq!(live.as_array().map(Vec::len), Some(1));
    let all = json_at(&home, &["workspace", "list", "--all"], 0);
    assert_eq!(all.as_array().map(Vec::len), Some(2));
    json_at(&home, &["workspace", "show", "nope"], 4);
}

// covers: C183
#[test]
fn allowed_roots_are_stored_and_an_empty_update_is_refused() {
    let home = home_with_workspace("roots");
    let set = json_at(
        &home,
        &[
            "workspace",
            "set",
            "acme",
            "--allowed-root",
            "/a",
            "--allowed-root",
            "/b",
        ],
        0,
    );
    assert_eq!(set["settings"], json!({"allowed_roots": ["/a", "/b"]}));
    let none = json_at(&home, &["workspace", "set", "acme"], 2);
    assert_eq!(none["error"]["message"], "nothing to update");
}

// covers: C109, C110, C113, C114
#[test]
fn a_project_is_created_shown_updated_and_listed() {
    let home = home_with_workspace("project");
    let created = json_at(
        &home,
        &["project", "create", "core-lib", "--description", "d"],
        0,
    );
    assert_eq!(created["name"], "Core Lib");
    assert_eq!(created["status"], "active");
    assert_eq!(created["repository_ids"], json!([]));
    json_at(&home, &["project", "create", "core-lib"], 3);
    let bad = json_at(&home, &["project", "create", "Bad_Slug"], 2);
    assert_eq!(bad["error"]["code"], "validation_failed");
    let updated = json_at(
        &home,
        &["project", "update", "core-lib", "--status", "archived"],
        0,
    );
    assert_eq!(updated["status"], "archived");
    let live = json_at(&home, &["project", "list"], 0);
    assert_eq!(live.as_array().map(Vec::len), Some(0));
    let all = json_at(&home, &["project", "list", "--all"], 0);
    assert_eq!(all[0]["slug"], "core-lib");
    let summary = json_at(&home, &["project", "show", "core-lib"], 0);
    assert_eq!(summary["open_tasks"], 0);
    json_at(&home, &["project", "show", "nope"], 4);
}

// covers: C112
#[test]
fn rename_term_rewrites_open_tasks_and_tags_and_dry_run_changes_nothing() {
    let home = home_with_workspace("rename");
    json_at(&home, &["project", "create", "rho", "--name", "Rho"], 0);
    let task = json_at(
        &home,
        &[
            "task",
            "create",
            "Port the Rho lexer",
            "--project",
            "rho",
            "--tag",
            "rho",
        ],
        0,
    );
    let key = task["key"].as_str().expect("a key").to_owned();
    let dry = json_at(
        &home,
        &[
            "project",
            "rename-term",
            "rho",
            "--from",
            "Rho",
            "--to",
            "Nyn",
            "--dry-run",
        ],
        0,
    );
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["tasks"][0]["key"], key);
    let untouched = json_at(&home, &["task", "show", &key], 0);
    assert_eq!(untouched["title"], "Port the Rho lexer");
    let done = json_at(
        &home,
        &[
            "project",
            "rename-term",
            "rho",
            "--from",
            "Rho",
            "--to",
            "Nyn",
        ],
        0,
    );
    assert_eq!(done["errors"], json!([]));
    let shown = json_at(&home, &["task", "show", &key], 0);
    assert_eq!(shown["title"], "Port the Nyn lexer");
    assert_eq!(shown["tags"], json!(["nyn"]));
    let project = json_at(&home, &["project", "show", "nyn"], 0);
    assert_eq!(project["project"]["name"], "Nyn");
    let again = json_at(
        &home,
        &[
            "project",
            "rename-term",
            "nyn",
            "--from",
            "Rho",
            "--to",
            "Nyn",
        ],
        0,
    );
    assert_eq!(again["tasks"], json!([]));
    let bad = json_at(
        &home,
        &[
            "project",
            "rename-term",
            "nyn",
            "--from",
            "R2",
            "--to",
            "Nyn",
        ],
        2,
    );
    assert_eq!(bad["error"]["code"], "validation_failed");
}

// covers: C112
#[test]
fn rename_term_reports_a_taken_slug_and_exits_1() {
    let home = home_with_workspace("rename-clash");
    json_at(&home, &["project", "create", "rho", "--name", "Rho"], 0);
    json_at(&home, &["project", "create", "nyn"], 0);
    let report = json_at(
        &home,
        &[
            "project",
            "rename-term",
            "rho",
            "--from",
            "Rho",
            "--to",
            "Nyn",
        ],
        1,
    );
    assert_eq!(report["errors"][0]["target"], "project rho");
}
