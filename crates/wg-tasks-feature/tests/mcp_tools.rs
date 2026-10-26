//! The task tools (M26 to M46) answer through `wg_tasks_feature::mcp::call` against a real database:
//! the structured output of a call, the `permission_denied` refusal of an agent without the
//! permission, the scope rules of `task_promote_ready`, and the names the feature leaves unanswered.

use std::collections::BTreeSet;
use std::error::Error;

use serde_json::{Value, json};
use wg_mcpspec::agent_permissions;
use wg_mcpspec::options::Agent;
use wg_model::entity::Workspace;
use wg_model::{Write, derive_id};
use wg_runtime::{App, human_context};
use wg_types::{ErrorCode, Timestamp, WgError};

type TestResult = Result<(), Box<dyn Error>>;

/// An application over a fresh home with the workspace `main` (prefix `WG`) and the task WG-1.
fn app(name: &str) -> Result<App, Box<dyn Error>> {
    let home = std::env::temp_dir().join(format!("wg-tasks-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let mut app = App::open(Some(&home))?;
    let at = Timestamp::parse_iso("2026-01-01T00:00:00Z")?;
    app.runtime
        .store
        .commit_decision(&wg_model::Decision::new(()).write(Write::insert(Workspace {
            id: derive_id(&[9; 32], 0),
            slug: "main".to_owned(),
            name: "Main".to_owned(),
            description: String::new(),
            key_prefix: "WG".to_owned(),
            next_task_number: 1,
            settings: serde_json::Map::new(),
            archived_at: None,
            created_at: at,
            updated_at: at,
        })))?;
    wg_tasks_feature::service::task_create(
        &mut app,
        &human_context(None)?,
        Some("main"),
        &json!({"title": "Pick a store"}),
    )?;
    Ok(app)
}

/// An agent holding the default agent permissions.
fn agent() -> Result<Agent, WgError> {
    Ok(Agent {
        name: "tester".to_owned(),
        permissions: agent_permissions(&[] as &[&str])?,
    })
}

/// An agent holding no permission at all.
fn bare_agent() -> Agent {
    Agent {
        name: "bare".to_owned(),
        permissions: BTreeSet::new(),
    }
}

/// The answer of tool `name` to `arguments`; a name the feature does not answer is an error here.
fn answer(app: &mut App, agent: &Agent, name: &str, arguments: &Value) -> Result<Value, WgError> {
    wg_tasks_feature::mcp::call(app, agent, name, arguments).unwrap_or_else(|| {
        Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("{name} not answered"),
        ))
    })
}

// covers: M26
#[test]
fn task_create_answers_the_created_task() -> TestResult {
    let mut app = app("create")?;
    let agent = agent()?;
    let out = answer(
        &mut app,
        &agent,
        "task_create",
        &json!({
            "title": "Store", "description": "", "acceptance_criteria": null,
            "priority": "medium", "ready": true, "tags": null, "files": null,
            "projects": null, "repositories": null, "parent": null,
            "depends_on": null, "workspace": "main"
        }),
    )?;
    assert_eq!(out["title"], json!("Store"));
    assert_eq!(out["status"], json!("ready"));
    assert_eq!(out["key"], json!("WG-2"));
    Ok(())
}

// covers: M27
#[test]
fn task_get_answers_the_task_by_key() -> TestResult {
    let mut app = app("get")?;
    let agent = agent()?;
    let out = answer(
        &mut app,
        &agent,
        "task_get",
        &json!({"task": "WG-1", "workspace": "main"}),
    )?;
    assert_eq!(out["key"], json!("WG-1"));
    assert_eq!(out["title"], json!("Pick a store"));
    Ok(())
}

// covers: M26
#[test]
fn task_create_is_refused_without_the_write_permission() -> TestResult {
    let mut app = app("denied")?;
    let refused = answer(
        &mut app,
        &bare_agent(),
        "task_create",
        &json!({"title": "No", "workspace": "main"}),
    );
    match refused {
        Err(err) => assert_eq!(err.code.as_str(), "permission_denied"),
        Ok(out) => panic!("expected a refusal, got {out}"),
    }
    Ok(())
}

// covers: M36
#[test]
fn task_promote_ready_needs_one_scope() -> TestResult {
    let mut app = app("scope")?;
    let agent = agent()?;
    let none = answer(
        &mut app,
        &agent,
        "task_promote_ready",
        &json!({"tags": null, "project": null, "all": false, "dry_run": true}),
    );
    match none {
        Err(err) => assert_eq!(err.code.as_str(), "validation_failed"),
        Ok(out) => panic!("expected a scope error, got {out}"),
    }
    let both = answer(
        &mut app,
        &agent,
        "task_promote_ready",
        &json!({"tags": ["x"], "project": null, "all": true, "dry_run": true}),
    );
    assert!(both.is_err(), "all cannot be combined with tags");
    let all = answer(
        &mut app,
        &agent,
        "task_promote_ready",
        &json!({"tags": null, "project": null, "all": true, "dry_run": true, "workspace": "main"}),
    )?;
    assert_eq!(all["dry_run"], json!(true));
    Ok(())
}

#[test]
fn task_search_and_contract_check_are_not_answered_yet() -> TestResult {
    let mut app = app("unanswered")?;
    let agent = agent()?;
    for name in ["task_search", "task_contract_check", "attempt_get"] {
        let out = wg_tasks_feature::mcp::call(&mut app, &agent, name, &json!({}));
        assert!(out.is_none(), "{name} must be left to the other features");
    }
    Ok(())
}
