//! IF-24: `task accept` and `task reject`, run through the real binary against a real SQLite
//! database and, for the stack landing, a real git repository. The workspace, the repository and
//! the stack are written through the store, because `init`, `repo register` and `stack` belong to
//! other groups.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use wg_model::entity::{
    Repository, RepositoryCheckout, Stack, StackItem, StackRepository, Workspace,
};
use wg_model::event::{EventDraft, EventPayload, StackVerified, StrMap};
use wg_model::{Decision, Write, derive_id};
use wg_runtime::{App, system_context};
use wg_types::{AccessMode, Id, StackItemState, StackStatus, Timestamp, parse_id};

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-if24-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn at() -> Timestamp {
    Timestamp::parse_iso("2026-01-01T00:00:00Z").expect("a timestamp")
}

/// A home with the workspace `acme` (prefix `AC`).
fn home_with_workspace(name: &str) -> (PathBuf, Id) {
    let home = temp_home(name);
    let id = derive_id(&[24; 32], 0);
    let mut app = App::open(Some(&home)).expect("the app opens");
    app.runtime
        .store
        .commit_decision(&Decision::new(()).write(Write::insert(Workspace {
            id,
            slug: "acme".to_owned(),
            name: "Acme".to_owned(),
            description: String::new(),
            key_prefix: "AC".to_owned(),
            next_task_number: 1,
            settings: serde_json::Map::new(),
            archived_at: None,
            created_at: at(),
            updated_at: at(),
        })))
        .expect("the workspace is written");
    (home, id)
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

/// A task that is claimed and completed, so it awaits review.
fn awaiting(home: &Path, title: &str, extra: &[&str]) {
    let mut create = vec!["task", "create", title, "--ready"];
    create.extend_from_slice(extra);
    json(home, &create, 0);
}

// covers: C145, C161
#[test]
fn accept_needs_verification_or_a_manual_reason_and_reject_returns_to_ready() {
    let (home, _) = home_with_workspace("plain");
    awaiting(&home, "One", &[]);
    json(&home, &["task", "claim", "AC-1"], 0);
    // Not complete yet: nothing to accept.
    json(
        &home,
        &["task", "accept", "AC-1", "--manual", "--reason", "x"],
        3,
    );
    json(&home, &["task", "complete", "AC-1"], 0);
    // No verification is configured, so only a manual acceptance with a reason can accept.
    let refused = json(&home, &["task", "accept", "AC-1"], 2);
    assert_eq!(refused["error"]["code"], "validation_failed");
    let blank = json(
        &home,
        &["task", "accept", "AC-1", "--manual", "--reason", " "],
        2,
    );
    assert_eq!(
        blank["error"]["message"],
        "manual acceptance requires a recorded reason"
    );
    let rejected = json(&home, &["task", "reject", "AC-1", "--reason", "again"], 0);
    assert_eq!(rejected["decision"], "rejected");
    assert_eq!(json(&home, &["task", "show", "AC-1"], 0)["status"], "ready");
    json(&home, &["task", "claim", "AC-1"], 0);
    json(&home, &["task", "complete", "AC-1"], 0);
    let accepted = json(
        &home,
        &["task", "accept", "AC-1", "--manual", "--reason", "reviewed"],
        0,
    );
    assert_eq!(accepted["mode"], "manual");
    assert_eq!(accepted["decision"], "accepted");
    assert_eq!(
        json(&home, &["task", "show", "AC-1"], 0)["status"],
        "accepted"
    );
    // The decisions are recorded oldest first.
    let (_, stdout, _) = run(&home, &["task", "show", "AC-1"]);
    assert!(stdout.contains("accepted"));
}

// covers: C145
#[test]
fn accept_promotes_the_dependents_that_become_ready_to_start() {
    let (home, _) = home_with_workspace("promote");
    awaiting(&home, "First", &[]);
    json(
        &home,
        &["task", "create", "Second", "--depends-on", "AC-1"],
        0,
    );
    json(&home, &["task", "claim", "AC-1"], 0);
    json(&home, &["task", "complete", "AC-1"], 0);
    let accepted = json(
        &home,
        &[
            "task",
            "accept",
            "AC-1",
            "--manual",
            "--reason",
            "reviewed",
            "--promote-dependents",
        ],
        0,
    );
    assert_eq!(accepted["promoted"], serde_json::json!(["AC-2"]));
    assert_eq!(json(&home, &["task", "show", "AC-2"], 0)["status"], "ready");
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A repository on `main` with an integration branch one commit ahead; returns (repo, base, tip).
fn repository_with_integration(name: &str) -> (PathBuf, String, String) {
    let repo = temp_home(name).join("svc");
    std::fs::create_dir_all(&repo).expect("the directory is made");
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "# svc\n").expect("a file");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "initial"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "-b", "wg/stack/one"]);
    std::fs::write(repo.join("a.py"), "A = 1\n").expect("a file");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "child work"]);
    let tip = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "main"]);
    (repo, base, tip)
}

/// Writes a verified stack of one verified child under AC-1 in `repo`.
fn seed_stack(home: &Path, workspace: Id, repo: &Path, base: &str, tip: &str) -> Id {
    let ids = |slot: u32| derive_id(&[25; 32], slot);
    let task_id = |key: &str| {
        let shown = json(home, &["task", "show", key], 0);
        parse_id(shown["id"].as_str().expect("an id")).expect("a uuid")
    };
    let (parent, child) = (task_id("AC-1"), task_id("AC-2"));
    let (repository, stack) = (ids(0), ids(1));
    let mut app = App::open(Some(home)).expect("the app opens");
    let context = system_context("test").expect("a context");
    let stamp = app.runtime.stamps.stamp(&context).expect("a stamp");
    let verified = EventDraft::new(
        &stamp,
        ids(9),
        Some(workspace),
        stack,
        EventPayload::StackVerified(StackVerified {
            run_id: None,
            tips: StrMap(std::iter::once(("svc".to_owned(), tip.to_owned())).collect()),
            tasks: std::iter::empty().collect(),
            reason: "verified".to_owned(),
            tolerated: None,
        }),
    );
    let decision = Decision::new(())
        .write(Write::insert(Repository {
            id: repository,
            workspace_id: workspace,
            name: "svc".to_owned(),
            description: String::new(),
            identity_fingerprint: None,
            default_branch: Some("main".to_owned()),
            tracked_branches: std::iter::empty().collect(),
            access_mode: AccessMode::ReadWrite,
            health_status: "ok".to_owned(),
            health_detail: serde_json::Map::new(),
            health_checked_at: None,
            is_shallow: false,
            last_scanned_at: None,
            archived_at: None,
            metadata: serde_json::Map::new(),
            created_at: at(),
            updated_at: at(),
        }))
        .write(Write::insert(RepositoryCheckout {
            id: ids(2),
            repository_id: repository,
            path: repo.to_string_lossy().into_owned(),
            kind: "clone".to_owned(),
            is_primary: true,
            access_mode: None,
            available: true,
            last_seen_at: None,
            created_at: at(),
            updated_at: at(),
        }))
        .write(Write::insert(Stack {
            id: stack,
            workspace_id: workspace,
            parent_task_id: parent,
            status: StackStatus::Verified,
            created_at: at(),
            updated_at: at(),
            land_request: None,
        }))
        .write(Write::insert(StackRepository {
            stack_id: stack,
            repository_id: repository,
            base_sha: base.to_owned(),
            integration_branch: "wg/stack/one".to_owned(),
            created_at: at(),
        }))
        .write(Write::insert(StackItem {
            id: ids(3),
            stack_id: stack,
            task_id: child,
            state: StackItemState::Verified,
            merge_order: Some(1),
            detail: Some(format!(
                r#"{{"conflicts": {{}}, "merges": {{"svc": {{"head": "{tip}", "merge_commit": "{tip}"}}}}}}"#
            )),
            created_at: at(),
            updated_at: at(),
        }))
        .event(verified);
    app.runtime
        .store
        .commit_decision(&decision)
        .expect("the stack is written");
    stack
}

// covers: C145
#[test]
fn accepting_the_parent_lands_the_verified_stack_and_a_dirty_checkout_refuses() {
    let (home, workspace) = home_with_workspace("landing");
    let (repo, base, tip) = repository_with_integration("landing-repo");
    awaiting(&home, "Parent", &[]);
    awaiting(&home, "Child", &["--parent", "AC-1"]);
    json(&home, &["task", "claim", "AC-1"], 0);
    json(&home, &["task", "complete", "AC-1"], 0);
    json(&home, &["task", "claim", "AC-2"], 0);
    json(&home, &["task", "complete", "AC-2"], 0);
    seed_stack(&home, workspace, &repo, &base, &tip);

    // A dirty user checkout refuses the landing and moves no ref.
    std::fs::write(repo.join("scratch.txt"), "not committed\n").expect("a file");
    let refused = json(
        &home,
        &["task", "accept", "AC-1", "--manual", "--reason", "ok"],
        2,
    );
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("uncommitted changes")),
        "{refused}"
    );
    assert_eq!(git(&repo, &["rev-parse", "main"]), base);
    assert_eq!(
        json(&home, &["task", "show", "AC-1"], 0)["status"],
        "awaiting_review"
    );

    std::fs::remove_file(repo.join("scratch.txt")).expect("the file goes");
    let accepted = json(
        &home,
        &["task", "accept", "AC-1", "--manual", "--reason", "ok"],
        0,
    );
    assert_eq!(accepted["mode"], "manual");
    assert_eq!(git(&repo, &["rev-parse", "main"]), tip);
    assert!(
        repo.join("a.py").exists(),
        "the checkout holds the landed work"
    );
    assert_eq!(
        json(&home, &["task", "show", "AC-1"], 0)["status"],
        "accepted"
    );
    assert_eq!(
        json(&home, &["task", "show", "AC-2"], 0)["status"],
        "accepted"
    );
    assert!(
        git(&repo, &["branch", "--list", "wg/stack/one"]).is_empty(),
        "the integration branch is removed"
    );
}
