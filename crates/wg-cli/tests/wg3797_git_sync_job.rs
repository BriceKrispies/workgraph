//! WG-3797: the `git_sync` job, run by the real worker against a real git repository through the
//! binary, and the stderr line a failed job writes (Python's `log.exception("job failed")`).

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-wg3797-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temp dir");
    dir
}

fn git(path: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
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

fn enqueue(home: &Path, kind: &str, payload: &Value) -> String {
    let body = payload.to_string();
    let queued = json_run(home, &["job", "enqueue", kind, "--payload", &body], 0);
    queued["job_id"].as_str().expect("a job id").to_owned()
}

fn worker_once(home: &Path) -> String {
    let (code, stdout, stderr) = run(home, &["worker", "run", "--once"]);
    assert_eq!(code, 0, "worker run: {stdout}{stderr}");
    stderr
}

#[test]
fn git_sync_job_syncs_the_repository_and_is_rerunnable() {
    let (root, home) = setup("sync");
    let repo = make_repo(&root, "svc");
    let registered = json_run(
        &home,
        &["repo", "register", repo.to_str().expect("utf-8")],
        0,
    );
    // Registration queues its own git_sync; drain it first so the two syncs never ingest the same
    // commits at once (the loser would fail on the unique (repository, sha) index and retry).
    worker_once(&home);
    let repository_id = registered["id"]
        .as_str()
        .expect("a repository id")
        .to_owned();

    let job = enqueue(
        &home,
        "git_sync",
        &json!({"repository_id": repository_id, "mode": "full"}),
    );
    worker_once(&home);
    let shown = json_run(&home, &["job", "show", &job], 0);
    assert_eq!(shown["status"], "succeeded", "{shown}");
    assert_eq!(shown["result"]["repository_id"], repository_id.as_str());
    assert_eq!(shown["result"]["repository"], "svc");
    assert_eq!(shown["result"]["mode"], "full");
    assert_eq!(shown["result"]["head_refs"], 1);

    // A repeated incremental sync finds every commit known: nothing new is ingested.
    let again = enqueue(&home, "git_sync", &json!({"repository_id": repository_id}));
    worker_once(&home);
    let shown_again = json_run(&home, &["job", "show", &again], 0);
    assert_eq!(shown_again["status"], "succeeded", "{shown_again}");
    assert_eq!(shown_again["result"]["mode"], "incremental");
    assert_eq!(shown_again["result"]["new_commits"], 0);
}

#[test]
fn git_sync_job_for_an_unknown_repository_is_dead_without_a_retry() {
    let (_root, home) = setup("missing");
    let job = enqueue(
        &home,
        "git_sync",
        &json!({"repository_id": "no-such-repository"}),
    );
    let stderr = worker_once(&home);
    let shown = json_run(&home, &["job", "show", &job], 0);
    assert_eq!(shown["status"], "dead", "{shown}");
    assert_eq!(shown["attempts"], 1);
    assert_eq!(shown["error_kind"], "not_found");
    assert_eq!(
        shown["last_error"],
        "repository no-such-repository not found"
    );
    // A domain error is not an unexpected exception, so the worker logs nothing about it.
    assert!(!stderr.contains("job failed"), "{stderr}");
}

#[test]
fn a_failed_job_logs_job_failed_to_stderr() {
    let (_root, home) = setup("failed-log");
    let job = enqueue(&home, "noop", &json!({"fail": "always"}));
    let stderr = worker_once(&home);
    assert!(stderr.contains("\"msg\": \"job failed\""), "{stderr}");
    assert!(stderr.contains("\"kind\": \"noop\""), "{stderr}");
    assert!(stderr.contains("RuntimeError: always"), "{stderr}");
    let shown = json_run(&home, &["job", "show", &job], 0);
    assert_eq!(shown["error_kind"], "exception");
}
