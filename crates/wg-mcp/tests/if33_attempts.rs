//! IF-33: the attempt tools answer from a real database, through the backend the server calls.
//!
//! The home is opened as the server opens it (migrated to head), one workspace, one task and one
//! attempt are written through the store, then the tools are called by name with the arguments
//! the catalog validates. The fixtures follow `wg-store`'s run tests.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;

use serde_json::{Map, Value, json};
use wg_mcp::{Agent, AppBackend, ToolBackend};
use wg_mcpspec::agent_permissions;
use wg_model::entity::{AgentAttempt, Task, Workspace};
use wg_model::{Write, derive_id};
use wg_runtime::App;
use wg_types::{
    AttemptStatus, Id, Permission, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError,
};

type TestResult = Result<(), Box<dyn Error>>;

/// Unwraps a fixture value that is valid by construction.
/// The exec feature, as the binary lists it: the attempt tools are answered by its table.
static EXEC: &[&wg_runtime::Feature] = &[&wg_exec_feature::FEATURE];

fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn id(n: u32) -> Id {
    derive_id(&entropy(9), n)
}

fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 26, 9, 0, second, 0))
}

fn workspace() -> Workspace {
    Workspace {
        id: id(0),
        slug: "default".into(),
        name: "Default".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 10,
        settings: Map::new(),
        archived_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

fn task() -> Task {
    Task {
        id: id(1),
        workspace_id: id(0),
        number: 7,
        key: ok(TaskKey::parse("WG-7")),
        title: "attempt task".into(),
        description: String::new(),
        description_hash: "h".into(),
        acceptance_criteria: Default::default(),
        status: TaskStatus::Running,
        priority: TaskPriority::Medium,
        category: None,
        parent_id: None,
        assignee: None,
        assigned_agent: None,
        claimed_by: None,
        claim_expires_at: None,
        auto_execute: false,
        estimated_effort: None,
        metadata: Map::new(),
        due_at: None,
        scheduled_at: None,
        created_by: "human:brice".into(),
        version: 1,
        created_at: at(1),
        updated_at: at(1),
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    }
}

fn attempt() -> AgentAttempt {
    AgentAttempt {
        id: id(2),
        workspace_id: id(0),
        task_id: id(1),
        number: 1,
        status: AttemptStatus::Running,
        provider: "recorder".into(),
        adapter: "recorder".into(),
        agent_id: Some("agent:recorder".into()),
        model: None,
        session_id: None,
        parent_attempt_id: None,
        retry_of_attempt_id: None,
        rerun_of_attempt_id: None,
        created_by: "agent:recorder".into(),
        created_at: at(2),
        queued_at: None,
        started_at: None,
        finished_at: None,
        queue_wait_ms: None,
        duration_ms: None,
        exit_reason: None,
        exit_code: None,
        error: None,
        summary: None,
        input_task_version: Some(1),
        input_task_snapshot: Map::new(),
        prompt_artifact_id: None,
        config_snapshot: Map::new(),
        environment: Map::new(),
        baseline_manifest_id: None,
        outcome_manifest_id: None,
        usage: Map::new(),
        usage_source: None,
        cost_usd: None,
        cost_source: None,
        telemetry: Map::new(),
        modified_repository_ids: Default::default(),
        retries: 0,
        cancel_requested: false,
        updated_at: at(2),
    }
}

/// A home with one workspace, one task (WG-7) and one running attempt of it.
fn seeded_app(name: &str) -> Result<(App, PathBuf), Box<dyn Error>> {
    let home = std::env::temp_dir().join(format!("wg-mcp-if33-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let mut app = App::open(Some(home.as_path()))?;
    app.runtime.store.write(|session| {
        session.apply_write(&Write::insert(workspace()))?;
        session.apply_write(&Write::insert(task()))?;
        session.apply_write(&Write::insert(attempt()))
    })?;
    Ok((app, home))
}

fn agent(permissions: BTreeSet<Permission>) -> Agent {
    Agent {
        name: "recorder".into(),
        permissions,
    }
}

fn granted() -> Result<BTreeSet<Permission>, WgError> {
    agent_permissions::<&str>(&[])
}

fn error_of(result: Result<Value, WgError>) -> (String, String) {
    match result {
        Ok(value) => panic!("expected an error, got {value}"),
        Err(err) => (err.code.as_str().to_owned(), err.message),
    }
}

// covers: M50
#[test]
fn attempt_get_returns_the_attempt() -> TestResult {
    let (app, home) = seeded_app("get")?;
    let mut backend = AppBackend {
        app,
        features: EXEC,
    };
    let out = backend.call_tool(
        &agent(granted()?),
        "attempt_get",
        json!({"attempt_id": id(2).to_string()}),
    )?;
    assert_eq!(out["id"], json!(id(2).to_string()));
    assert_eq!(out["task_id"], json!(id(1).to_string()));
    assert_eq!(out["number"], json!(1));
    assert_eq!(out["status"], json!("running"));
    assert_eq!(out["provider"], json!("recorder"));
    let _ = std::fs::remove_dir_all(home);
    Ok(())
}

// covers: M50
#[test]
fn attempt_get_names_an_unknown_attempt() -> TestResult {
    let (app, home) = seeded_app("get-missing")?;
    let mut backend = AppBackend {
        app,
        features: EXEC,
    };
    let (code, message) = error_of(backend.call_tool(
        &agent(granted()?),
        "attempt_get",
        json!({"attempt_id": "no-such-attempt"}),
    ));
    assert_eq!(code, "not_found");
    assert_eq!(message, "attempt 'no-such-attempt' not found");
    let _ = std::fs::remove_dir_all(home);
    Ok(())
}

// covers: M51
#[test]
fn attempt_list_pages_and_filters() -> TestResult {
    let (app, home) = seeded_app("list")?;
    let mut backend = AppBackend {
        app,
        features: EXEC,
    };
    let all = backend.call_tool(
        &agent(granted()?),
        "attempt_list",
        json!({"limit": 10, "offset": 0}),
    )?;
    assert_eq!(all["total"], json!(1));
    assert_eq!(all["items"][0]["id"], json!(id(2).to_string()));
    assert_eq!(all["next_offset"], Value::Null);

    let by_task = backend.call_tool(
        &agent(granted()?),
        "attempt_list",
        json!({"task": "wg-7", "limit": 10, "offset": 0, "statuses": ["running"]}),
    )?;
    assert_eq!(by_task["total"], json!(1));

    let unknown_status = backend.call_tool(
        &agent(granted()?),
        "attempt_list",
        json!({"statuses": ["bogus"], "limit": 10, "offset": 0}),
    )?;
    assert_eq!(unknown_status["total"], json!(0));
    assert_eq!(unknown_status["items"], json!([]));
    let _ = std::fs::remove_dir_all(home);
    Ok(())
}

// covers: M50
#[test]
fn a_missing_permission_is_refused_before_the_read() -> TestResult {
    let (app, home) = seeded_app("permission")?;
    let mut backend = AppBackend {
        app,
        features: EXEC,
    };
    let (code, message) = error_of(backend.call_tool(
        &agent(BTreeSet::new()),
        "attempt_get",
        json!({"attempt_id": id(2).to_string()}),
    ));
    assert_eq!(code, "permission_denied");
    assert_eq!(
        message,
        "actor 'agent:recorder' lacks permission(s): task:read"
    );
    let _ = std::fs::remove_dir_all(home);
    Ok(())
}

// covers: M48
#[test]
fn a_task_reference_that_names_nothing_is_not_found() -> TestResult {
    let (app, home) = seeded_app("task-missing")?;
    let mut backend = AppBackend {
        app,
        features: EXEC,
    };
    let mut arguments = Map::new();
    arguments.insert("task".into(), json!("DEMO-999"));
    let (code, message) = error_of(backend.call_tool(
        &agent(granted()?),
        "attempt_start",
        Value::Object(arguments),
    ));
    assert_eq!(code, "not_found");
    assert_eq!(message, "task 'DEMO-999' not found");
    let _ = std::fs::remove_dir_all(home);
    Ok(())
}

#[test]
fn a_known_task_resolves_and_the_unwired_success_path_says_so() -> TestResult {
    let (app, home) = seeded_app("task-known")?;
    let mut backend = AppBackend {
        app,
        features: EXEC,
    };
    let (code, message) = error_of(backend.call_tool(
        &agent(granted()?),
        "git_get_changes",
        json!({"task": "WG-7"}),
    ));
    assert_eq!(code, "workgraph_error");
    assert_eq!(message, "tool git_get_changes is not wired yet");
    let _ = std::fs::remove_dir_all(home);
    Ok(())
}

// covers: M55
#[test]
fn an_ambiguous_task_key_is_refused_without_a_workspace() -> TestResult {
    let (mut app, home) = seeded_app("ambiguous")?;
    // A second workspace holding its own WG-7, so the key names two tasks.
    let mut other = workspace();
    other.id = id(5);
    other.slug = "other".into();
    let mut other_task = task();
    other_task.id = id(6);
    other_task.workspace_id = id(5);
    app.runtime.store.write(|session| {
        session.apply_write(&Write::insert(other))?;
        session.apply_write(&Write::insert(other_task))
    })?;
    let mut backend = AppBackend {
        app,
        features: EXEC,
    };

    let (code, message) = error_of(backend.call_tool(
        &agent(granted()?),
        "git_get_changes",
        json!({"task": "WG-7"}),
    ));
    assert_eq!(code, "validation_failed");
    assert_eq!(
        message,
        "task key 'WG-7' is ambiguous across workspaces; pass a workspace"
    );

    // Naming the workspace picks its task, and the reference then resolves.
    for workspace in ["default", "other"] {
        let (code, message) = error_of(backend.call_tool(
            &agent(granted()?),
            "git_get_changes",
            json!({"task": "WG-7", "workspace": workspace}),
        ));
        assert_eq!(code, "workgraph_error", "workspace {workspace}");
        assert_eq!(message, "tool git_get_changes is not wired yet");
    }
    let _ = std::fs::remove_dir_all(home);
    Ok(())
}
