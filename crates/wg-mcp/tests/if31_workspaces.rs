//! IF-31: the workspace, project and repository tools (M01 to M08) against a real SQLite home and
//! real Git repositories in temporary directories.
//!
//! Each test opens its own home, seeds one workspace and one project through the store, and calls
//! the tools through the same `ToolBackend::call_tool` the server uses. The invalid-arguments path
//! goes through `respond_to_call`, as the server does.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use wg_mcp::{Agent, AppBackend, ToolBackend};
use wg_mcpspec::{Catalog, agent_permissions, respond_to_call};
use wg_model::entity::{Project, Workspace};
use wg_model::{Row, Write};
use wg_runtime::{App, new_id};
use wg_types::{ErrorCode, Timestamp, WgError};

/// One home, its backend and the agent calling it. The directory is removed on drop, best effort
/// (Windows keeps the database open until the backend drops).
struct Fixture {
    backend: AppBackend,
    agent: Agent,
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("wg-mcp-if31-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).expect("root");
        let app = App::open(Some(&root.join("home"))).expect("open the home");
        let mut backend = AppBackend(app);
        let now = Timestamp::from_components(2026, 10, 26, 0, 0, 0, 0).expect("timestamp");
        let workspace = Workspace {
            id: new_id().expect("id"),
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            description: String::new(),
            key_prefix: "DEMO".to_owned(),
            next_task_number: 1,
            settings: Map::new(),
            archived_at: None,
            created_at: now,
            updated_at: now,
        };
        let project = Project {
            id: new_id().expect("id"),
            workspace_id: workspace.id,
            slug: "platform".to_owned(),
            name: "Platform".to_owned(),
            description: String::new(),
            status: "active".to_owned(),
            settings: Map::new(),
            created_at: now,
            updated_at: now,
        };
        backend
            .0
            .runtime
            .store
            .write(|session| {
                session.apply_write(&Write::insert(Row::Workspace(workspace.clone())))?;
                session.apply_write(&Write::insert(Row::Project(project.clone())))
            })
            .expect("seed the workspace and its project");
        let agent = Agent {
            name: "if31-test".to_owned(),
            permissions: agent_permissions(&["repo:register"]).expect("permissions"),
        };
        Fixture {
            backend,
            agent,
            root,
        }
    }

    fn call(&mut self, name: &str, arguments: Value) -> Result<Value, WgError> {
        self.backend.call_tool(&self.agent, name, arguments)
    }

    /// The path a registration of `dir` stores: canonical, without the verbatim prefix.
    fn stored(&self, dir: &Path) -> String {
        std::fs::canonicalize(dir)
            .expect("canonical")
            .display()
            .to_string()
            .trim_start_matches(r"\\?\")
            .to_owned()
    }

    /// A Git repository with one commit at `root/<name>`.
    fn repo(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::create_dir_all(&path).expect("repo dir");
        git(&path, &["init", "-q"]);
        std::fs::write(path.join("README.md"), "fixture\n").expect("readme");
        git(&path, &["add", "README.md"]);
        git(&path, &["commit", "-q", "-m", "Initial commit"]);
        path
    }

    /// Registers `dir` under `name` and returns the `RepositoryOut`.
    fn register(&mut self, dir: &Path, name: &str) -> Value {
        self.call(
            "repository_register",
            json!({"path": dir.display().to_string(), "name": name}),
        )
        .expect("register")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "if31")
        .env("GIT_AUTHOR_EMAIL", "if31@example.invalid")
        .env("GIT_COMMITTER_NAME", "if31")
        .env("GIT_COMMITTER_EMAIL", "if31@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_not_found(error: &WgError, message: &str) {
    assert_eq!(error.code, ErrorCode::NotFound, "{error:?}");
    assert_eq!(error.message, message);
}

// covers: M01
#[test]
fn workspace_list_answers_and_refuses_bad_arguments() {
    let mut fx = Fixture::new("m01");
    let listed = fx.call("workspace_list", json!({})).expect("list");
    let slugs: Vec<&str> = listed["workspaces"]
        .as_array()
        .expect("list")
        .iter()
        .filter_map(|ws| ws["slug"].as_str())
        .collect();
    assert_eq!(slugs, ["demo"]);

    // The arguments are checked before the tool runs: an array is not an object.
    let catalog = Catalog::embedded().expect("catalog");
    let reply = respond_to_call(&catalog, "workspace_list", Some(&json!([])), |valid| {
        fx.call("workspace_list", valid)
    });
    assert_eq!(reply["isError"], json!(true));
}

// covers: M02
#[test]
fn workspace_get_counts_the_queue_and_refuses_an_unknown_workspace() {
    let mut fx = Fixture::new("m02");
    let repo = fx.repo("svc");
    fx.register(&repo, "svc");
    let summary = fx
        .call("workspace_get", json!({"workspace": "demo"}))
        .expect("get");
    assert_eq!(summary["workspace"]["slug"], json!("demo"));
    assert_eq!(summary["projects"], json!(1));
    assert_eq!(summary["repositories"], json!(1));
    // Registration queues the repository's git_sync job.
    assert_eq!(summary["queued_jobs"], json!(1));
    assert_eq!(summary["dead_jobs"], json!(0));
    assert_eq!(summary["running_attempts"], json!(0));

    let error = fx
        .call("workspace_get", json!({"workspace": "no-such-workspace"}))
        .expect_err("unknown");
    assert_not_found(&error, "workspace 'no-such-workspace' not found");
}

// covers: M03
#[test]
fn project_list_answers_and_refuses_an_unknown_workspace() {
    let mut fx = Fixture::new("m03");
    let listed = fx.call("project_list", json!({})).expect("list");
    assert_eq!(listed["projects"][0]["slug"], json!("platform"));
    assert_eq!(listed["projects"][0]["repository_ids"], json!([]));

    let error = fx
        .call("project_list", json!({"workspace": "no-such-workspace"}))
        .expect_err("unknown");
    assert_not_found(&error, "workspace 'no-such-workspace' not found");
}

// covers: M04
#[test]
fn project_get_summarises_and_refuses_an_unknown_project() {
    let mut fx = Fixture::new("m04");
    let summary = fx
        .call("project_get", json!({"project": "platform"}))
        .expect("get");
    assert_eq!(summary["project"]["slug"], json!("platform"));
    assert_eq!(summary["repositories"], json!([]));
    assert_eq!(summary["tasks_by_status"], json!({}));
    assert_eq!(summary["open_tasks"], json!(0));
    assert_eq!(summary["accepted_tasks"], json!(0));
    assert_eq!(summary["recent_tasks"], json!([]));

    let error = fx
        .call("project_get", json!({"project": "no-such-project"}))
        .expect_err("unknown");
    assert_not_found(&error, "project 'no-such-project' not found in workspace");
}

// covers: M05
#[test]
fn repository_list_lists_registrations_and_refuses_an_unknown_project() {
    let mut fx = Fixture::new("m05");
    let repo = fx.repo("svc");
    fx.register(&repo, "svc");
    let listed = fx.call("repository_list", json!({})).expect("list");
    let repositories = listed["repositories"].as_array().expect("list");
    assert_eq!(repositories.len(), 1);
    assert_eq!(repositories[0]["name"], json!("svc"));
    assert_eq!(repositories[0]["access_mode"], json!("read_write"));
    assert_eq!(
        repositories[0]["checkouts"][0]["path"],
        json!(fx.stored(&repo))
    );

    // Nothing is in the project, so the filtered list is empty.
    let in_project = fx
        .call("repository_list", json!({"project": "platform"}))
        .expect("filtered");
    assert_eq!(in_project["repositories"], json!([]));

    let error = fx
        .call("repository_list", json!({"project": "no-such-project"}))
        .expect_err("unknown");
    assert_not_found(&error, "project 'no-such-project' not found in workspace");
}

// covers: M06
#[test]
fn repository_get_answers_by_name_and_refuses_an_unknown_repository() {
    let mut fx = Fixture::new("m06");
    let repo = fx.repo("svc");
    let registered = fx.register(&repo, "svc");
    let got = fx
        .call("repository_get", json!({"repository": "svc"}))
        .expect("get");
    assert_eq!(got["id"], registered["id"]);
    assert!(!got["default_branch"].is_null());
    assert_eq!(got["checkouts"].as_array().map(Vec::len), Some(1));

    let error = fx
        .call("repository_get", json!({"repository": "no-such-repo"}))
        .expect_err("unknown");
    assert_not_found(&error, "repository 'no-such-repo' not found in workspace");
}

// covers: M07
#[test]
fn repository_register_joins_by_path_and_refuses_bad_paths_and_names() {
    let mut fx = Fixture::new("m07");
    let repo = fx.repo("svc");
    let first = fx.register(&repo, "svc");
    assert_eq!(first["name"], json!("svc"));
    assert_eq!(first["checkouts"][0]["kind"], json!("clone"));
    assert_eq!(first["checkouts"][0]["is_primary"], json!(true));

    // The same path again joins the same repository: no second row, no second sync job.
    let again = fx.register(&repo, "svc");
    assert_eq!(again["id"], first["id"]);
    let summary = fx
        .call("workspace_get", json!({"workspace": "demo"}))
        .expect("get");
    assert_eq!(summary["repositories"], json!(1));
    assert_eq!(summary["queued_jobs"], json!(1));

    // A directory that is not a repository.
    let plain = fx.root.join("plain");
    std::fs::create_dir_all(&plain).expect("plain dir");
    let error = fx
        .call(
            "repository_register",
            json!({"path": plain.display().to_string()}),
        )
        .expect_err("not a repository");
    assert_eq!(error.code, ErrorCode::ValidationFailed);
    assert_eq!(
        error.message,
        format!("{} is not a Git repository", plain.display())
    );

    // A path that does not exist.
    let missing = fx.root.join("missing");
    let error = fx
        .call(
            "repository_register",
            json!({"path": missing.display().to_string()}),
        )
        .expect_err("missing");
    assert_eq!(error.code, ErrorCode::ValidationFailed);
    assert_eq!(
        error.message,
        format!(
            "path {} does not exist or is not a directory",
            missing.display()
        )
    );

    // An explicit name already taken by another repository is a conflict.
    let other = fx.repo("other");
    let error = fx
        .call(
            "repository_register",
            json!({"path": other.display().to_string(), "name": "svc"}),
        )
        .expect_err("name taken");
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(error.message, "repository name 'svc' is already registered");
}

// covers: M08
#[test]
fn repository_status_reports_the_working_tree_and_refuses_an_unknown_repository() {
    let mut fx = Fixture::new("m08");
    let repo = fx.repo("svc");
    fx.register(&repo, "svc");
    std::fs::write(repo.join("untracked.txt"), "new\n").expect("untracked file");

    let status = fx
        .call("repository_status", json!({"repository": "svc"}))
        .expect("status");
    assert_eq!(status["name"], json!("svc"));
    assert_eq!(status["available"], json!(true));
    assert_eq!(status["checkout_path"], json!(fx.stored(&repo)));
    assert!(status["head"].is_string());
    assert_eq!(status["untracked"], json!(["untracked.txt"]));
    assert_eq!(status["staged"], json!([]));
    assert_eq!(status["unstaged"], json!([]));

    let error = fx
        .call("repository_status", json!({"repository": "no-such-repo"}))
        .expect_err("unknown");
    assert_not_found(&error, "repository 'no-such-repo' not found in workspace");
}
