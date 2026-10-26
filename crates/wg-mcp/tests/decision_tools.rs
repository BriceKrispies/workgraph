//! The decision tools answer from a real database, through the catalog's argument checks and the
//! backend the server calls. The documents were checked against the Python server: the same calls
//! on both give the same `structuredContent`, text and `isError`.

use std::error::Error;

use serde_json::{Value, json};
use wg_mcp::{Agent, AppBackend, ToolBackend};

/// The features the backend serves: the decisions feature.
static FEATURES: &[&wg_runtime::Feature] = &[&wg_decisions::FEATURE];
use wg_mcpspec::{Catalog, agent_permissions, respond_to_call};
use wg_model::Write;
use wg_model::entity::Workspace;
use wg_runtime::{App, human_context};
use wg_types::Timestamp;

type TestResult = Result<(), Box<dyn Error>>;

/// A backend over a home with the workspace `main` (prefix `WG`) and the task WG-1.
fn backend(name: &str) -> Result<AppBackend, Box<dyn Error>> {
    let home = std::env::temp_dir().join(format!("wg-mcp-decisions-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let mut app = App::open(Some(&home))?;
    let at = Timestamp::parse_iso("2026-01-01T00:00:00Z")?;
    app.runtime
        .store
        .commit_decision(&wg_model::Decision::new(()).write(Write::insert(Workspace {
            id: wg_model::derive_id(&[7; 32], 0),
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
    app.task_create(
        &human_context(None)?,
        Some("main"),
        &json!({"title": "Pick a store"}),
    )?;
    Ok(AppBackend {
        app,
        features: FEATURES,
    })
}

/// Calls `name` as the server does: argument checks, then the backend. Returns the result value.
fn call(backend: &mut AppBackend, name: &str, arguments: &Value) -> Result<Value, Box<dyn Error>> {
    let agent = Agent {
        name: "tester".to_owned(),
        permissions: agent_permissions::<&str>(&[])?,
    };
    let catalog = Catalog::embedded()?;
    Ok(respond_to_call(&catalog, name, Some(arguments), |valid| {
        backend.call_tool(&agent, name, valid)
    }))
}

// covers: M69, M67, M68
#[test]
fn request_list_and_read_back_a_decision() -> TestResult {
    let mut backend = backend("round-trip")?;
    let made = call(
        &mut backend,
        "task_request_decision",
        &json!({"task": "WG-1", "question": "Q1?", "options": [
            {"label": "A", "detail": "d", "recommended": true}, {"label": "B", "id": "bee"}]}),
    )?;
    assert_eq!(made["isError"], json!(false));
    let decision = &made["structuredContent"];
    assert_eq!(decision["created_by"], "agent:tester");
    assert_eq!(decision["options"][0]["id"], "a");
    assert_eq!(decision["options"][1]["id"], "bee");
    assert_eq!(decision["options"][1]["detail"], "");

    let listed = call(&mut backend, "decision_list", &json!({}))?;
    assert_eq!(
        listed["structuredContent"]["decisions"][0]["id"],
        decision["id"]
    );
    assert_eq!(
        listed["structuredContent"]["decisions"][0]["task"]["key"],
        "WG-1"
    );

    let for_task = call(&mut backend, "task_decisions", &json!({"task": "WG-1"}))?;
    assert_eq!(
        for_task["structuredContent"]["decisions"][0]["status"],
        "open"
    );
    Ok(())
}

// covers: M69, M67, M68
#[test]
fn bad_calls_are_error_results() -> TestResult {
    let mut backend = backend("errors")?;
    for (name, arguments) in [
        (
            "task_request_decision",
            json!({"task": "WG-9", "question": "Q"}),
        ),
        (
            "task_request_decision",
            json!({"task": "WG-1", "question": ""}),
        ),
        (
            "task_request_decision",
            json!({"task": "WG-1", "question": "Q", "options": [
                {"label": "a", "id": "x"}, {"label": "b", "id": "x"}]}),
        ),
        ("decision_list", json!({"status": "bogus"})),
        ("decision_list", json!({"workspace": "nope"})),
        ("task_decisions", json!({"task": "WG-9"})),
    ] {
        let result = call(&mut backend, name, &arguments)?;
        assert_eq!(result["isError"], json!(true), "{name} {arguments}");
    }
    Ok(())
}
