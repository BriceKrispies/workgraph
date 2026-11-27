//! Setting, changing and clearing a task's parent after creation, through MCP `task_update` and
//! the CLI `task update --parent` / `--clear-parent`. A cycle, a nonexistent parent and a parent
//! in another workspace are refused with the create-side validation, and the change is recorded
//! in the task's history.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;

use serde_json::{Value, json};
use wg_iface::mcp::options::Agent;
use wg_registry_feature::service::registry::workspace_create;
use wg_runtime::{App, human_context};
use wg_tasks_feature::mcp;
use wg_tasks_feature::service;
use wg_tasks_feature::spec::args::{command, update_body};
use wg_types::Permission;

type TestResult = Result<(), Box<dyn Error>>;

/// A fresh app with the workspace `main` (key prefix `WG`).
fn app(name: &str) -> Result<App, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("task_parent")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let mut app = App::open(Some(&dir))?;
    let human = human_context(None)?;
    let body = json!({"slug": "main", "name": "Main", "description": "", "key_prefix": "WG"});
    workspace_create(&mut app, &human, &body, true)?;
    Ok(app)
}

fn create_body(title: &str) -> Value {
    json!({
        "title": title, "description": "", "acceptance_criteria": ["it works"],
        "priority": "medium", "status": "backlog", "tags": [], "files": [],
        "projects": [], "repositories": [], "depends_on": [], "auto_execute": false,
    })
}

/// Creates a task with no parent in `workspace` (the default when `None`).
fn create(app: &mut App, workspace: Option<&str>, title: &str) -> TestResult {
    let context = human_context(None)?;
    service::task_create_from(app, &context, workspace, &create_body(title), true)?;
    Ok(())
}

/// An MCP call as a context-free agent with the task permissions; errors read `code: message`.
fn mcp(app: &mut App, name: &str, arguments: Value) -> Result<Value, String> {
    let agent = Agent {
        name: "claude-code".to_owned(),
        permissions: BTreeSet::from([
            Permission::WorkspaceRead,
            Permission::TaskRead,
            Permission::TaskWrite,
        ]),
    };
    match mcp::call(app, &agent, name, &arguments) {
        Some(Ok(value)) => Ok(value),
        Some(Err(error)) => Err(format!("{}: {}", error.code.as_str(), error.message)),
        None => Err(format!("no MCP tool {name}")),
    }
}

fn mcp_update(app: &mut App, key: &str, arguments: Value) -> Result<Value, String> {
    let mut args = arguments;
    args["task"] = json!(key);
    mcp(app, "task_update", args)
}

fn no_file(_: &str) -> Result<String, String> {
    Err("no file read in this test".to_owned())
}

/// `workgraph task update KEY ARGS...`, parsed by the task group's clap command and run through
/// the service the command calls.
fn cli_update(app: &mut App, key: &str, options: &[&str]) -> Result<(), String> {
    let mut argv = vec!["task", "update", key];
    argv.extend_from_slice(options);
    let matches = command()
        .try_get_matches_from(argv)
        .map_err(|error| error.to_string())?;
    let update = matches
        .subcommand_matches("update")
        .ok_or("no update subcommand")?;
    let body = update_body(update, &no_file).map_err(|error| format!("{error:?}"))?;
    let context = human_context(None).map_err(|error| error.to_string())?;
    service::task_update(app, &context, None, key, &body)
        .map(|_| ())
        .map_err(|error| format!("{}: {}", error.code.as_str(), error.message))
}

/// The task's stored parent id, or `None`.
fn parent_id(app: &mut App, key: &str) -> Result<Option<String>, String> {
    let task = mcp(app, "task_get", json!({"task": key}))?;
    Ok(task["parent_id"].as_str().map(str::to_owned))
}

/// The ids of the task's children.
fn child_ids(app: &mut App, key: &str) -> Result<Vec<String>, String> {
    let task = mcp(app, "task_get", json!({"task": key}))?;
    Ok(task["child_ids"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

fn id_of(app: &mut App, key: &str) -> Result<String, String> {
    let task = mcp(app, "task_get", json!({"task": key}))?;
    task["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{key} has no id"))
}

/// Three tasks, WG-1 to WG-3, none with a parent.
fn three_tasks(app: &mut App) -> TestResult {
    for title in ["First", "Second", "Third"] {
        create(app, None, title)?;
    }
    Ok(())
}

#[test]
fn mcp_sets_changes_and_clears_the_parent() -> TestResult {
    let mut app = app("mcp_set_change_clear")?;
    three_tasks(&mut app)?;
    let child_id = id_of(&mut app, "WG-3")?;

    mcp_update(&mut app, "WG-3", json!({"parent": "WG-1"}))?;
    assert_eq!(parent_id(&mut app, "WG-3")?, Some(id_of(&mut app, "WG-1")?));
    assert!(child_ids(&mut app, "WG-1")?.contains(&child_id));

    // A change moves the child from the old parent's list to the new one.
    mcp_update(&mut app, "WG-3", json!({"parent": "WG-2"}))?;
    assert_eq!(parent_id(&mut app, "WG-3")?, Some(id_of(&mut app, "WG-2")?));
    assert!(child_ids(&mut app, "WG-2")?.contains(&child_id));
    assert!(!child_ids(&mut app, "WG-1")?.contains(&child_id));

    mcp_update(&mut app, "WG-3", json!({"clear_parent": true}))?;
    assert_eq!(parent_id(&mut app, "WG-3")?, None);
    assert!(!child_ids(&mut app, "WG-2")?.contains(&child_id));
    Ok(())
}

#[test]
fn cli_sets_changes_and_clears_the_parent() -> TestResult {
    let mut app = app("cli_set_change_clear")?;
    three_tasks(&mut app)?;
    let child_id = id_of(&mut app, "WG-3")?;

    cli_update(&mut app, "WG-3", &["--parent", "WG-1"])?;
    assert_eq!(parent_id(&mut app, "WG-3")?, Some(id_of(&mut app, "WG-1")?));
    assert!(child_ids(&mut app, "WG-1")?.contains(&child_id));

    cli_update(&mut app, "WG-3", &["--parent", "WG-2"])?;
    assert_eq!(parent_id(&mut app, "WG-3")?, Some(id_of(&mut app, "WG-2")?));
    assert!(!child_ids(&mut app, "WG-1")?.contains(&child_id));

    cli_update(&mut app, "WG-3", &["--clear-parent"])?;
    assert_eq!(parent_id(&mut app, "WG-3")?, None);
    assert!(!child_ids(&mut app, "WG-2")?.contains(&child_id));
    Ok(())
}

#[test]
fn a_parent_cycle_is_refused_and_named_on_mcp_and_cli() -> TestResult {
    let mut app = app("cycle")?;
    three_tasks(&mut app)?;
    // WG-2 under WG-1, WG-3 under WG-2: the chain is WG-3 -> WG-2 -> WG-1.
    mcp_update(&mut app, "WG-2", json!({"parent": "WG-1"}))?;
    mcp_update(&mut app, "WG-3", json!({"parent": "WG-2"}))?;

    // Making WG-3 the parent of WG-1 would close the loop WG-1 -> WG-3 -> WG-2 -> WG-1.
    let refusal = mcp_update(&mut app, "WG-1", json!({"parent": "WG-3"}))
        .err()
        .unwrap_or_default();
    assert!(refusal.starts_with("dependency_cycle"), "{refusal}");
    assert!(
        refusal.contains("WG-1 -> WG-3 -> WG-2 -> WG-1"),
        "{refusal}"
    );

    let refusal = cli_update(&mut app, "WG-1", &["--parent", "WG-3"])
        .err()
        .unwrap_or_default();
    assert!(refusal.starts_with("dependency_cycle"), "{refusal}");
    assert!(
        refusal.contains("WG-1 -> WG-3 -> WG-2 -> WG-1"),
        "{refusal}"
    );

    // A task cannot be its own parent.
    let refusal = cli_update(&mut app, "WG-1", &["--parent", "WG-1"])
        .err()
        .unwrap_or_default();
    assert!(refusal.starts_with("dependency_cycle"), "{refusal}");

    // Refused changes leave the parent column alone.
    assert_eq!(parent_id(&mut app, "WG-1")?, None);
    Ok(())
}

#[test]
fn a_nonexistent_parent_is_refused_on_mcp_and_cli() -> TestResult {
    let mut app = app("nonexistent")?;
    three_tasks(&mut app)?;

    let refusal = mcp_update(&mut app, "WG-3", json!({"parent": "WG-99"}))
        .err()
        .unwrap_or_default();
    assert!(refusal.starts_with("not_found"), "{refusal}");
    assert!(refusal.contains("WG-99"), "{refusal}");

    let refusal = cli_update(&mut app, "WG-3", &["--parent", "WG-99"])
        .err()
        .unwrap_or_default();
    assert!(refusal.starts_with("not_found"), "{refusal}");
    assert!(refusal.contains("WG-99"), "{refusal}");
    assert_eq!(parent_id(&mut app, "WG-3")?, None);
    Ok(())
}

#[test]
fn a_parent_in_another_workspace_is_refused() -> TestResult {
    let mut app = app("other_workspace")?;
    three_tasks(&mut app)?;
    let human = human_context(None)?;
    let other = json!({"slug": "other", "name": "Other", "description": "", "key_prefix": "OT"});
    workspace_create(&mut app, &human, &other, true)?;
    create(&mut app, Some("other"), "Elsewhere")?;

    let refusal = mcp_update(&mut app, "WG-3", json!({"parent": "OT-1"}))
        .err()
        .unwrap_or_default();
    assert!(refusal.starts_with("not_found"), "{refusal}");
    assert!(refusal.contains("OT-1"), "{refusal}");
    Ok(())
}

#[test]
fn giving_a_parent_and_clearing_it_together_is_refused() -> TestResult {
    let mut app = app("both_flags")?;
    three_tasks(&mut app)?;
    mcp_update(&mut app, "WG-3", json!({"parent": "WG-1"}))?;

    let refusal = mcp_update(
        &mut app,
        "WG-3",
        json!({"parent": "WG-2", "clear_parent": true}),
    )
    .err()
    .unwrap_or_default();
    assert!(refusal.contains("not both"), "{refusal}");
    assert_eq!(parent_id(&mut app, "WG-3")?, Some(id_of(&mut app, "WG-1")?));
    Ok(())
}

#[test]
fn the_parent_change_is_recorded_in_history() -> TestResult {
    let mut app = app("history")?;
    three_tasks(&mut app)?;
    mcp_update(&mut app, "WG-3", json!({"parent": "WG-1"}))?;
    mcp_update(&mut app, "WG-3", json!({"clear_parent": true}))?;

    let history = mcp(&mut app, "task_get_history", json!({"task": "WG-3"}))?;
    let text = history.to_string();
    assert!(text.contains("parent_id"), "{text}");
    Ok(())
}
