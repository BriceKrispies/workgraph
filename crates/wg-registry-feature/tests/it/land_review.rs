//! WG-5453: git sync moves a task to `awaiting_review` when a commit carrying its `Task-ID`
//! trailer is reachable from the repository's integration branch. Each test builds a temporary
//! git repository, registers it in a fresh home and syncs it through the services the CLI calls.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use wg_git_feature::service::git_sync;
use wg_registry::admin::Patch;
use wg_registry_feature::service::{
    repo_integration_branch, repo_register, repo_update, workspace_create, workspace_set_roots,
};
use wg_registry_feature::spec::registry::workspace_create_body;
use wg_registry_feature::spec::repos::RegisterInput;
use wg_runtime::{App, RequestContext, human_context};
use wg_store::{Raw, Statement};
use wg_tasks_feature::service::{task_create, task_update};

const WS: Option<&str> = Some("t");

struct Fixture {
    app: App,
    ctx: RequestContext,
    repo: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Tester",
            "-c",
            "user.email=tester@example.com",
        ])
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// Commits a new file on the current branch and returns the commit's SHA.
fn commit(dir: &Path, file: &str, message: &str) -> String {
    std::fs::write(dir.join(file), file).expect("write file");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

impl Fixture {
    /// A home with one workspace (`t`, prefix `WG`) and one registered repository whose `main`
    /// and `integration` branches share an initial commit; `integration` is checked out.
    fn new(name: &str) -> Self {
        let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("land_review")
            .join(name);
        if home.exists() {
            std::fs::remove_dir_all(&home).expect("clear scratch");
        }
        let repo = home.join("repos").join("repo");
        std::fs::create_dir_all(&repo).expect("create repo dir");
        git(&repo, &["init", "-q", "-b", "main"]);
        commit(&repo, "base.txt", "initial");
        git(&repo, &["branch", "integration"]);
        git(&repo, &["checkout", "-q", "integration"]);
        let mut app = App::open(Some(&home)).expect("open app");
        let ctx = human_context(Some("tester")).expect("context");
        let body = workspace_create_body("t", Some("T"), "WG", "");
        workspace_create(&mut app, &ctx, &body, true).expect("workspace");
        let input = RegisterInput {
            path: repo.display().to_string(),
            ..RegisterInput::default()
        };
        repo_register(&mut app, &ctx, WS, &input).expect("register");
        Fixture { app, ctx, repo }
    }

    /// Creates the next task (keys are WG-1, WG-2, ... in creation order) and moves it to `status`.
    fn task(&mut self, title: &str, status: &str) {
        let body = json!({"title": title, "status": "ready"});
        task_create(&mut self.app, &self.ctx, WS, &body).expect("create task");
        if status != "ready" {
            let key = format!("WG-{}", self.count("SELECT COUNT(*) FROM tasks", &[]));
            let path: &[&str] = match status {
                "awaiting_review" => &["running", "awaiting_review"],
                "failed" => &["running", "failed"],
                _ => std::slice::from_ref(&status),
            };
            for step in path {
                self.set_status(&key, step);
            }
        }
    }

    fn set_status(&mut self, key: &str, status: &str) {
        task_update(
            &mut self.app,
            &self.ctx,
            WS,
            key,
            &json!({"status": status}),
        )
        .expect("update status");
    }

    fn set_integration(&mut self, branch: &str) {
        let patch = Patch {
            integration_branch: Some(branch.to_owned()),
            ..Patch::default()
        };
        repo_update(&mut self.app, &self.ctx, WS, "repo", &patch).expect("set integration branch");
    }

    fn sync(&mut self, full: bool) {
        git_sync(&mut self.app, &self.ctx, WS, Some("repo"), full).expect("sync");
    }

    fn rows(&mut self, sql: &str, params: &[&str]) -> Vec<Vec<Raw>> {
        let statement = Statement {
            sql: sql.to_owned(),
            params: params.iter().map(|p| Raw::Text((*p).to_owned())).collect(),
        };
        self.app
            .runtime
            .store
            .read(|session| session.rows(&statement))
            .expect("query")
    }

    fn count(&mut self, sql: &str, params: &[&str]) -> i64 {
        match self.rows(sql, params).first().and_then(|row| row.first()) {
            Some(Raw::Int(n)) => *n,
            other => panic!("not a count: {other:?}"),
        }
    }

    fn status(&mut self, key: &str) -> String {
        match self
            .rows("SELECT status FROM tasks WHERE key = ?1", &[key])
            .first()
            .and_then(|row| row.first())
        {
            Some(Raw::Text(text)) => text.clone(),
            other => panic!("no status for {key}: {other:?}"),
        }
    }

    /// The comments the guardrail left on a task.
    fn land_comments(&mut self, key: &str) -> i64 {
        self.count(
            "SELECT COUNT(*) FROM task_comments c JOIN tasks t ON t.id = c.task_id \
             WHERE t.key = ?1 AND c.data LIKE '%landed_commit%'",
            &[key],
        )
    }

    /// The transitions to `awaiting_review` recorded for a task.
    fn review_transitions(&mut self, key: &str) -> i64 {
        self.count(
            "SELECT COUNT(*) FROM events e JOIN tasks t ON t.id = e.entity_id \
             WHERE t.key = ?1 AND e.event_type = 'task.status_changed' \
             AND e.data LIKE '%awaiting_review%'",
            &[key],
        )
    }
}

fn trailer(key: &str) -> String {
    format!("work for {key}\n\nTask-ID: {key}")
}

#[test]
fn landing_moves_open_tasks_to_awaiting_review_once() {
    let mut f = Fixture::new("moves");
    f.task("backlog", "backlog");
    f.task("ready", "ready");
    f.task("blocked", "blocked");
    f.task("claimed", "claimed");
    f.task("running", "running");
    f.set_integration("integration");
    let repo = f.repo.clone();
    let mut shas = Vec::new();
    for n in 1..=5 {
        let file = format!("f{n}.txt");
        shas.push(commit(&repo, &file, &trailer(&format!("WG-{n}"))));
    }
    f.sync(false);
    for n in 1..=5 {
        let key = format!("WG-{n}");
        assert_eq!(f.status(&key), "awaiting_review", "{key}");
        assert_eq!(f.land_comments(&key), 1, "{key} comment");
        assert_eq!(f.review_transitions(&key), 1, "{key} transition");
        let named = f.count(
            "SELECT COUNT(*) FROM task_comments c JOIN tasks t ON t.id = c.task_id \
             WHERE t.key = ?1 AND c.body LIKE ?2 AND c.body LIKE '%integration%'",
            &[&key, &format!("%{}%", shas[n - 1])],
        );
        assert_eq!(named, 1, "{key} comment names the commit and the branch");
    }
    // Syncing again, incrementally and in full, records nothing more.
    f.sync(false);
    f.sync(true);
    for n in 1..=5 {
        let key = format!("WG-{n}");
        assert_eq!(f.land_comments(&key), 1, "{key} comment after resync");
        assert_eq!(
            f.review_transitions(&key),
            1,
            "{key} transition after resync"
        );
    }
}

#[test]
fn a_commit_only_on_a_feature_branch_does_not_move_the_task_until_it_is_merged() {
    let mut f = Fixture::new("feature");
    f.task("ready", "ready");
    f.set_integration("integration");
    let repo = f.repo.clone();
    git(&repo, &["checkout", "-q", "-b", "feature"]);
    commit(&repo, "feature.txt", &trailer("WG-1"));
    f.sync(false);
    assert_eq!(f.status("WG-1"), "ready");
    assert_eq!(f.land_comments("WG-1"), 0);

    git(&repo, &["checkout", "-q", "integration"]);
    git(
        &repo,
        &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"],
    );
    f.sync(false);
    assert_eq!(f.status("WG-1"), "awaiting_review");
    assert_eq!(f.land_comments("WG-1"), 1);
    assert_eq!(f.review_transitions("WG-1"), 1);
}

#[test]
fn finished_tasks_and_body_mentions_are_not_changed() {
    let mut f = Fixture::new("untouched");
    f.task("cancelled", "cancelled");
    f.task("failed", "failed");
    f.task("in review", "awaiting_review");
    f.task("mentioned", "ready");
    f.task("part of", "ready");
    f.set_integration("integration");
    let repo = f.repo.clone();
    for n in 1..=3 {
        let file = format!("f{n}.txt");
        commit(&repo, &file, &trailer(&format!("WG-{n}")));
    }
    commit(&repo, "g4.txt", "mentions WG-4 in prose\n\nRefs: WG-4");
    commit(&repo, "g5.txt", "a part\n\nPart-of: WG-5");
    f.sync(false);
    assert_eq!(f.status("WG-1"), "cancelled");
    assert_eq!(f.status("WG-2"), "failed");
    assert_eq!(f.status("WG-3"), "awaiting_review");
    assert_eq!(f.status("WG-4"), "ready");
    assert_eq!(f.status("WG-5"), "ready");
    for n in 1..=5 {
        assert_eq!(f.land_comments(&format!("WG-{n}")), 0, "WG-{n}");
    }
}

#[test]
fn without_an_integration_branch_nothing_moves() {
    let mut f = Fixture::new("unset");
    f.task("ready", "ready");
    let repo = f.repo.clone();
    commit(&repo, "f.txt", &trailer("WG-1"));
    f.sync(false);
    assert_eq!(f.status("WG-1"), "ready");
    assert_eq!(f.land_comments("WG-1"), 0);
}

#[test]
fn the_workspace_switch_turns_the_guardrail_off() {
    let mut f = Fixture::new("switch");
    f.task("ready", "ready");
    f.set_integration("integration");
    workspace_set_roots(&mut f.app, &f.ctx, "t", &[], Some(false)).expect("switch off");
    let repo = f.repo.clone();
    commit(&repo, "f.txt", &trailer("WG-1"));
    f.sync(false);
    assert_eq!(f.status("WG-1"), "ready");

    workspace_set_roots(&mut f.app, &f.ctx, "t", &[], Some(true)).expect("switch on");
    f.sync(false);
    assert_eq!(f.status("WG-1"), "awaiting_review");
    assert_eq!(f.land_comments("WG-1"), 1);
}

#[test]
fn a_task_reopened_after_landing_is_not_moved_again_by_the_same_commit() {
    let mut f = Fixture::new("reopened");
    f.task("ready", "ready");
    f.set_integration("integration");
    let repo = f.repo.clone();
    commit(&repo, "f.txt", &trailer("WG-1"));
    f.sync(false);
    assert_eq!(f.status("WG-1"), "awaiting_review");
    f.set_status("WG-1", "ready");
    f.sync(false);
    assert_eq!(f.status("WG-1"), "ready");
    assert_eq!(f.land_comments("WG-1"), 1);
}

#[test]
fn the_integration_branch_is_set_shown_and_cleared() {
    let mut f = Fixture::new("setting");
    let shown = |f: &mut Fixture| {
        repo_integration_branch(&mut f.app, &f.ctx, WS, "repo").expect("read setting")
    };
    assert_eq!(shown(&mut f), None);
    f.set_integration("integration");
    assert_eq!(shown(&mut f).as_deref(), Some("integration"));
    f.set_integration("");
    assert_eq!(shown(&mut f), None);
}
