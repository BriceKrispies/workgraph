//! WG-3822: `repository_register` requires `repo:register`, as Python's `RepositoryService.register`
//! does. Without the grant the call is refused with `permission_denied` and nothing is written;
//! with the grant the same call registers the repository.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use wg_mcp::{Agent, AppBackend, ToolBackend};
use wg_mcpspec::agent_permissions;
use wg_model::entity::{Project, Workspace};
use wg_model::{Row, Write};
use wg_runtime::{App, Feature, new_id};
use wg_types::{Permission, Timestamp, WgError};

/// The features the backend serves: the registry feature.
static REGISTRY: &[&Feature] = &[&wg_registry_feature::FEATURE];

/// A home with one seeded workspace, its backend, and a temporary directory removed on drop.
struct Fixture {
    backend: AppBackend,
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "wg-mcp-wg3822-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("root");
        let app = App::open(Some(&root.join("home"))).expect("open the home");
        let mut backend = AppBackend {
            app,
            features: REGISTRY,
        };
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
            .app
            .runtime
            .store
            .write(|session| {
                session.apply_write(&Write::insert(Row::Workspace(workspace.clone())))?;
                session.apply_write(&Write::insert(Row::Project(project.clone())))
            })
            .expect("seed the workspace and its project");
        Fixture { backend, root }
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
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Windows keeps the database open until the backend drops; removal is best effort.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "wg3822")
        .env("GIT_AUTHOR_EMAIL", "wg3822@example.invalid")
        .env("GIT_COMMITTER_NAME", "wg3822")
        .env("GIT_COMMITTER_EMAIL", "wg3822@example.invalid")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// An MCP agent with the default grants plus `grants`.
fn agent(grants: &[&str]) -> Agent {
    Agent {
        name: "wg3822-test".to_owned(),
        permissions: agent_permissions(grants).expect("permissions"),
    }
}

/// The error code and message of a refused call.
fn error_of(result: Result<Value, WgError>) -> (String, String) {
    match result {
        Ok(value) => panic!("expected a refusal, got {value}"),
        Err(err) => (err.code.as_str().to_owned(), err.message),
    }
}

// covers: M07
#[test]
fn repository_register_is_refused_without_repo_register() {
    let mut fx = Fixture::new("refused");
    let repo = fx.repo("svc");
    let arguments = json!({"path": repo.display().to_string(), "name": "svc"});

    // The default agent grants do not include repo:register.
    let (code, message) = error_of(fx.backend.call_tool(
        &agent(&[]),
        "repository_register",
        arguments.clone(),
    ));
    assert_eq!(code, "permission_denied");
    assert_eq!(
        message,
        "actor 'agent:wg3822-test' lacks permission(s): repo:register"
    );

    // The refusal writes nothing: the repository is not registered.
    let listed = fx
        .backend
        .call_tool(&agent(&[]), "repository_list", json!({}))
        .expect("list");
    assert_eq!(listed["repositories"], json!([]));
}

// covers: M07
#[test]
fn repository_register_succeeds_with_repo_register() {
    let mut fx = Fixture::new("granted");
    let repo = fx.repo("svc");
    let arguments = json!({"path": repo.display().to_string(), "name": "svc"});

    let registered = fx
        .backend
        .call_tool(&agent(&["repo:register"]), "repository_register", arguments)
        .expect("register with the grant");
    assert_eq!(registered["name"], json!("svc"));
    assert_eq!(registered["checkouts"][0]["is_primary"], json!(true));
}

/// An MCP agent with exactly `grants`, without the default grants, so a refusal names the one
/// permission the tool needs and no other.
fn bare_agent(grants: &[Permission]) -> Agent {
    Agent {
        name: "wg3822-test".to_owned(),
        permissions: grants.iter().copied().collect(),
    }
}

/// Calls `tool` as an agent with `grants` and expects `permission_denied` naming `missing`.
fn assert_refused(
    fx: &mut Fixture,
    tool: &str,
    arguments: Value,
    grants: &[Permission],
    missing: &str,
) {
    let (code, message) = error_of(fx.backend.call_tool(&bare_agent(grants), tool, arguments));
    assert_eq!(code, "permission_denied", "{tool}");
    assert_eq!(
        message,
        format!("actor 'agent:wg3822-test' lacks permission(s): {missing}"),
        "{tool}"
    );
}

// covers: M01
#[test]
fn workspace_list_is_refused_without_workspace_read() {
    let mut fx = Fixture::new("m01");
    assert_refused(&mut fx, "workspace_list", json!({}), &[], "workspace:read");
    assert_refused(
        &mut fx,
        "workspace_list",
        json!({}),
        &[Permission::RepoRead],
        "workspace:read",
    );
    let listed = fx
        .backend
        .call_tool(
            &bare_agent(&[Permission::WorkspaceRead]),
            "workspace_list",
            json!({}),
        )
        .expect("list with the grant");
    assert_eq!(listed["workspaces"].as_array().map(Vec::len), Some(1));
}

// covers: M02
#[test]
fn workspace_get_is_refused_without_workspace_read() {
    let mut fx = Fixture::new("m02");
    assert_refused(&mut fx, "workspace_get", json!({}), &[], "workspace:read");
    assert_refused(
        &mut fx,
        "workspace_get",
        json!({}),
        &[Permission::RepoRead],
        "workspace:read",
    );
}

// covers: M03
#[test]
fn project_list_is_refused_without_workspace_read() {
    let mut fx = Fixture::new("m03");
    assert_refused(&mut fx, "project_list", json!({}), &[], "workspace:read");
    assert_refused(
        &mut fx,
        "project_list",
        json!({}),
        &[Permission::RepoRead],
        "workspace:read",
    );
}

// covers: M04
#[test]
fn project_get_is_refused_without_workspace_read() {
    let mut fx = Fixture::new("m04");
    let arguments = json!({"project": "platform"});
    assert_refused(
        &mut fx,
        "project_get",
        arguments.clone(),
        &[],
        "workspace:read",
    );
    assert_refused(
        &mut fx,
        "project_get",
        arguments,
        &[Permission::RepoRead],
        "workspace:read",
    );
}

// covers: M05
#[test]
fn repository_list_is_refused_without_repo_read() {
    let mut fx = Fixture::new("m05");
    assert_refused(&mut fx, "repository_list", json!({}), &[], "repo:read");
    assert_refused(
        &mut fx,
        "repository_list",
        json!({}),
        &[Permission::WorkspaceRead],
        "repo:read",
    );
    let listed = fx
        .backend
        .call_tool(
            &bare_agent(&[Permission::RepoRead]),
            "repository_list",
            json!({}),
        )
        .expect("list with the grant");
    assert_eq!(listed["repositories"], json!([]));
}

// covers: M06
#[test]
fn repository_get_is_refused_without_repo_read() {
    let mut fx = Fixture::new("m06");
    let arguments = json!({"repository": "svc"});
    assert_refused(
        &mut fx,
        "repository_get",
        arguments.clone(),
        &[],
        "repo:read",
    );
    assert_refused(
        &mut fx,
        "repository_get",
        arguments,
        &[Permission::WorkspaceRead],
        "repo:read",
    );
}

// covers: M07
#[test]
fn repository_register_is_refused_without_repo_register_alone() {
    let mut fx = Fixture::new("m07");
    let arguments = json!({"path": fx.root.display().to_string()});
    // A read grant is not the write grant: only repo:register admits registration.
    assert_refused(
        &mut fx,
        "repository_register",
        arguments,
        &[Permission::RepoRead],
        "repo:register",
    );
}

// covers: M08
#[test]
fn repository_status_is_refused_without_repo_read() {
    let mut fx = Fixture::new("m08");
    let arguments = json!({"repository": "svc"});
    assert_refused(
        &mut fx,
        "repository_status",
        arguments.clone(),
        &[],
        "repo:read",
    );
    assert_refused(
        &mut fx,
        "repository_status",
        arguments,
        &[Permission::WorkspaceRead],
        "repo:read",
    );
}
