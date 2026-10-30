//! The stack tools (M09 to M16) answer through `wg_stacks_feature::mcp::call` against a real
//! database: the list wrappers, the `permission_denied` refusal of an agent without `stack:write`
//! (and of a context without `task:read`), the `not_found` of an unknown parent, and the names the
//! feature leaves to the other features.

use std::collections::BTreeSet;
use std::error::Error;

use serde_json::{Value, json};
use wg_iface::mcp::agent_permissions;
use wg_iface::mcp::options::Agent;
use wg_model::entity::Workspace;
use wg_model::{Decision, Write, derive_id};
use wg_runtime::App;
use wg_types::{ErrorCode, Timestamp, WgError};

type TestResult = Result<(), Box<dyn Error>>;

/// An application over a fresh home with the workspace `main` (prefix `WG`) and no stacks.
fn app(name: &str) -> Result<App, Box<dyn Error>> {
    let home = std::env::temp_dir().join(format!("wg-stacks-mcp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let mut app = App::open(Some(&home))?;
    let at = Timestamp::parse_iso("2026-01-01T00:00:00Z")?;
    app.runtime
        .store
        .commit_decision(&Decision::new(()).write(Write::insert(Workspace {
            id: derive_id(&[7; 32], 0),
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
    Ok(app)
}

/// An agent holding the default agent permissions, which include `task:read` but not `stack:write`.
fn agent() -> Result<Agent, WgError> {
    Ok(Agent {
        name: "tester".to_owned(),
        permissions: agent_permissions(&[] as &[&str])?,
    })
}

/// An agent holding `stack:write` on top of the defaults.
fn writer() -> Result<Agent, WgError> {
    Ok(Agent {
        name: "writer".to_owned(),
        permissions: agent_permissions(&["stack:write"])?,
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
    wg_stacks_feature::mcp::call(app, agent, name, arguments).unwrap_or_else(|| {
        Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("{name} not answered"),
        ))
    })
}

// covers: M10
#[test]
fn stack_list_wraps_the_empty_workspace_under_stacks() -> TestResult {
    let mut app = app("list")?;
    let out = answer(
        &mut app,
        &agent()?,
        "stack_list",
        &json!({"include_inactive": false}),
    )?;
    assert_eq!(out, json!({"stacks": []}));
    Ok(())
}

// covers: M10
#[test]
fn stack_list_takes_include_inactive_as_false_when_absent() -> TestResult {
    let mut app = app("list-default")?;
    let out = answer(&mut app, &agent()?, "stack_list", &json!({}))?;
    assert_eq!(out, json!({"stacks": []}));
    Ok(())
}

// covers: M10
#[test]
fn stack_list_is_refused_without_task_read() -> TestResult {
    let mut app = app("list-refused")?;
    let err = answer(&mut app, &bare_agent(), "stack_list", &json!({}))
        .err()
        .ok_or("stack_list answered an agent without task:read")?;
    assert_eq!(err.code.as_str(), "permission_denied");
    assert!(err.message.contains("task:read"), "{}", err.message);
    Ok(())
}

// covers: M09
#[test]
fn stack_get_of_an_unknown_parent_is_not_found() -> TestResult {
    let mut app = app("get-missing")?;
    let err = answer(
        &mut app,
        &agent()?,
        "stack_get",
        &json!({"parent": "WG-99"}),
    )
    .err()
    .ok_or("stack_get answered an unknown parent")?;
    assert_eq!(err.code.as_str(), "not_found");
    Ok(())
}

// covers: M11
#[test]
fn stack_history_of_an_unknown_parent_is_an_error() -> TestResult {
    let mut app = app("history-missing")?;
    let result = answer(
        &mut app,
        &agent()?,
        "stack_history",
        &json!({"parent": "WG-99"}),
    );
    assert!(result.is_err());
    Ok(())
}

// covers: M12
#[test]
fn stack_create_is_refused_without_stack_write() -> TestResult {
    let mut app = app("create-refused")?;
    let err = answer(
        &mut app,
        &agent()?,
        "stack_create",
        &json!({"parent": "WG-1"}),
    )
    .err()
    .ok_or("stack_create answered without stack:write")?;
    assert_eq!(err.code.as_str(), "permission_denied");
    assert_missing_stack_write(&err);
    Ok(())
}

// covers: M13
#[test]
fn stack_add_item_is_refused_without_stack_write() -> TestResult {
    let mut app = app("add-refused")?;
    let err = answer(
        &mut app,
        &agent()?,
        "stack_add_item",
        &json!({"parent": "WG-1", "task": "WG-2"}),
    )
    .err()
    .ok_or("stack_add_item answered without stack:write")?;
    assert_eq!(err.code.as_str(), "permission_denied");
    assert_missing_stack_write(&err);
    Ok(())
}

// covers: M14
#[test]
fn stack_remove_item_is_refused_without_stack_write() -> TestResult {
    let mut app = app("remove-refused")?;
    let err = answer(
        &mut app,
        &agent()?,
        "stack_remove_item",
        &json!({"parent": "WG-1", "task": "WG-2"}),
    )
    .err()
    .ok_or("stack_remove_item answered without stack:write")?;
    assert_eq!(err.code.as_str(), "permission_denied");
    assert_missing_stack_write(&err);
    Ok(())
}

// covers: M15
#[test]
fn stack_integrate_is_refused_without_stack_write() -> TestResult {
    let mut app = app("integrate-refused")?;
    let err = answer(
        &mut app,
        &agent()?,
        "stack_integrate",
        &json!({"parent": "WG-1"}),
    )
    .err()
    .ok_or("stack_integrate answered without stack:write")?;
    assert_eq!(err.code.as_str(), "permission_denied");
    assert_missing_stack_write(&err);
    Ok(())
}

// covers: M16
#[test]
fn stack_verify_is_refused_without_stack_write() -> TestResult {
    let mut app = app("verify-refused")?;
    let err = answer(
        &mut app,
        &agent()?,
        "stack_verify",
        &json!({"parent": "WG-1"}),
    )
    .err()
    .ok_or("stack_verify answered without stack:write")?;
    assert_eq!(err.code.as_str(), "permission_denied");
    assert_missing_stack_write(&err);
    Ok(())
}

// covers: M15
#[test]
fn stack_integrate_with_stack_write_reaches_the_parent_lookup() -> TestResult {
    let mut app = app("integrate-writer")?;
    let err = answer(
        &mut app,
        &writer()?,
        "stack_integrate",
        &json!({"parent": "WG-99"}),
    )
    .err()
    .ok_or("stack_integrate answered an unknown parent")?;
    assert_eq!(err.code.as_str(), "not_found");
    Ok(())
}

#[test]
fn a_tool_outside_the_stack_group_is_not_answered() -> TestResult {
    let mut app = app("not-ours")?;
    let answered =
        wg_stacks_feature::mcp::call(&mut app, &agent()?, "task_get", &json!({"task": "WG-1"}));
    assert!(answered.is_none());
    Ok(())
}

/// The refusal names `stack:write` as the missing permission.
fn assert_missing_stack_write(err: &WgError) {
    assert!(err.message.contains("stack:write"), "{}", err.message);
}
