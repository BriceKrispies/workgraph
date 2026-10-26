//! IF-04: the `repo register, list, show, refresh, status, update, remove, remove-checkout,
//! depend` commands, run through the real binary against a real SQLite database and real git
//! repositories.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-if04-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    dir
}

fn git(path: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .expect("git runs");
    assert!(status.status.success(), "git {args:?}: {status:?}");
}

/// A repository with one commit on `main`.
fn make_repo(root: &Path, name: &str) -> PathBuf {
    let path = root.join("repos").join(name);
    std::fs::create_dir_all(&path).expect("a repo dir");
    git(&path, &["init", "-q", "-b", "main"]);
    git(&path, &["config", "user.name", "Test Author"]);
    git(&path, &["config", "user.email", "author@example.com"]);
    git(&path, &["config", "core.autocrlf", "false"]);
    std::fs::write(path.join("README.md"), format!("# {name}\n")).expect("a file");
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "initial commit"]);
    path
}

fn run(home: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_workgraph"))
        .arg("--home")
        .arg(home)
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

fn json_run(home: &Path, args: &[&str], expect: i32) -> Value {
    let mut all = vec!["--json"];
    all.extend_from_slice(args);
    let (code, stdout, stderr) = run(home, &all);
    assert_eq!(code, expect, "{args:?}: {stdout}{stderr}");
    let text = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{args:?}: {e}: {text}"))
}

/// A home initialised with the workspace `acme`, and a scratch directory for repositories.
fn setup(name: &str) -> (PathBuf, PathBuf) {
    let root = temp_dir(name);
    let home = root.join("home");
    json_run(&home, &["init", "--workspace", "acme", "--prefix", "AC"], 0);
    (root, home)
}

fn text(path: &Path) -> &str {
    path.to_str().expect("a utf-8 path")
}

// covers: C118, C121, C116
#[test]
fn register_show_and_list_a_repository() {
    let (root, home) = setup("register");
    let repo = make_repo(&root, "svc");
    let registered = json_run(&home, &["repo", "register", text(&repo)], 0);
    assert_eq!(registered["name"], "svc");
    assert_eq!(registered["tracked_branches"], json!(["main"]));
    assert_eq!(registered["default_branch"], "main");
    assert_eq!(registered["checkouts"][0]["is_primary"], true);
    // Registering the same path again joins the same repository.
    let again = json_run(&home, &["repo", "register", text(&repo)], 0);
    assert_eq!(again["id"], registered["id"]);
    assert_eq!(again["checkouts"].as_array().map(Vec::len), Some(1));

    let shown = json_run(&home, &["repo", "show", "svc"], 0);
    assert_eq!(shown["id"], registered["id"]);
    let listed = json_run(&home, &["repo", "list"], 0);
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    let missing = json_run(&home, &["repo", "show", "ghost"], 4);
    assert_eq!(missing["error"]["code"], "not_found");

    let not_git = root.join("plain");
    std::fs::create_dir_all(&not_git).expect("a dir");
    let refused = json_run(&home, &["repo", "register", text(&not_git)], 2);
    assert_eq!(refused["error"]["code"], "validation_failed");
}

// covers: C123
#[test]
fn update_tracks_and_untracks_branches() {
    let (root, home) = setup("track");
    let repo = make_repo(&root, "svc");
    json_run(&home, &["repo", "register", text(&repo)], 0);

    let out = json_run(
        &home,
        &[
            "repo", "update", "svc", "--track", "release", "--track", "main", "--track", "feature",
        ],
        0,
    );
    assert_eq!(
        out["tracked_branches"],
        json!(["main", "release", "feature"])
    );
    let out = json_run(
        &home,
        &[
            "repo",
            "update",
            "svc",
            "--untrack",
            "release",
            "--track",
            "hotfix",
        ],
        0,
    );
    assert_eq!(
        out["tracked_branches"],
        json!(["main", "feature", "hotfix"])
    );
}

// covers: C123
#[test]
fn update_default_branch_change_tracks_the_new_default() {
    let (root, home) = setup("default");
    let repo = make_repo(&root, "svc");
    json_run(&home, &["repo", "register", text(&repo)], 0);
    json_run(&home, &["repo", "update", "svc", "--track", "feature"], 0);
    git(&repo, &["branch", "develop"]);

    let out = json_run(
        &home,
        &["repo", "update", "svc", "--default-branch", "develop"],
        0,
    );
    assert_eq!(out["default_branch"], "develop");
    assert_eq!(
        out["tracked_branches"],
        json!(["main", "feature", "develop"])
    );
    let out = json_run(
        &home,
        &["repo", "update", "svc", "--access", "read_only"],
        0,
    );
    assert_eq!(out["access_mode"], "read_only");
}

// covers: C115, C119, C120, C117, C122
#[test]
fn depend_remove_checkout_status_refresh_and_remove() {
    let (root, home) = setup("lifecycle");
    let alpha = make_repo(&root, "alpha");
    let beta = make_repo(&root, "beta");
    let a = json_run(&home, &["repo", "register", text(&alpha)], 0);
    let b = json_run(&home, &["repo", "register", text(&beta)], 0);

    let out = json_run(
        &home,
        &["repo", "depend", "alpha", "beta", "--description", "uses"],
        0,
    );
    assert_eq!(out["depends_on"], json!([b["id"]]));
    let twice = json_run(&home, &["repo", "depend", "alpha", "beta"], 0);
    assert_eq!(twice["depends_on"], json!([b["id"]]));
    let own = json_run(&home, &["repo", "depend", "alpha", "alpha"], 2);
    assert_eq!(own["error"]["code"], "validation_failed");
    let out = json_run(&home, &["repo", "depend", "alpha", "beta", "--remove"], 0);
    assert_eq!(out["depends_on"], json!([]));

    std::fs::write(alpha.join("untracked.txt"), "u\n").expect("a file");
    let status = json_run(&home, &["repo", "status", "alpha"], 0);
    assert_eq!(status["available"], true);
    assert_eq!(status["branch"], "main");
    assert_eq!(status["untracked"], json!(["untracked.txt"]));
    let refreshed = json_run(&home, &["repo", "refresh", "alpha"], 0);
    assert_eq!(refreshed["health_status"], "ok");

    let unknown = json_run(
        &home,
        &[
            "repo",
            "remove-checkout",
            "alpha",
            text(&root.join("nowhere")),
        ],
        4,
    );
    assert_eq!(unknown["error"]["code"], "not_found");
    let emptied = json_run(
        &home,
        &["repo", "remove-checkout", "alpha", text(&alpha)],
        0,
    );
    assert_eq!(emptied["checkouts"], json!([]));
    let status = json_run(&home, &["repo", "status", "alpha"], 0);
    assert_eq!(status["available"], false);
    assert_eq!(status["error"], "no available checkout");

    let removed = json_run(&home, &["repo", "remove", "beta"], 0);
    assert!(removed["archived_at"].is_string());
    assert_eq!(
        json_run(&home, &["repo", "list"], 0)
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        json_run(&home, &["repo", "list", "--all"], 0)
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    json_run(&home, &["repo", "remove", "beta"], 4);
    assert_eq!(a["name"], "alpha");
}

// covers: C121
#[test]
fn human_views_render_the_repository() {
    let (root, home) = setup("human");
    let repo = make_repo(&root, "svc");
    let (code, out, err) = run(&home, &["repo", "register", text(&repo)]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("svc"), "{out}");
    assert!(out.contains("default branch: main"), "{out}");
    assert!(out.contains("checkout"), "{out}");
    let (_, listed, _) = run(&home, &["repo", "list"]);
    assert!(listed.contains("primary path"), "{listed}");
    let (_, removed, _) = run(&home, &["repo", "remove", "svc"]);
    assert!(removed.contains("removed registration of svc; files on disk untouched"));
}
