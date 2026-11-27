//! `tasks.max_open_children`: a create under a parent that already has that many open children is
//! refused with `too_many_children` and the children listed; finished children free a place, a
//! create without a parent is never counted, and zero (the default) turns the check off.

use std::error::Error;
use std::path::PathBuf;

use serde_json::json;
use wg_registry_feature::service::registry::workspace_create;
use wg_runtime::{App, human_context};
use wg_tasks_feature::service;

type TestResult = Result<(), Box<dyn Error>>;

/// A fresh workspace `main`, with `tasks.max_open_children` set to `limit`.
fn app(name: &str, limit: u64) -> Result<App, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("child_limit")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let mut app = App::open(Some(&dir))?;
    let human = human_context(None)?;
    let body = json!({"slug": "main", "name": "Main", "description": "", "key_prefix": "WG"});
    workspace_create(&mut app, &human, &body, true)?;
    let mut config = serde_json::Map::new();
    config.insert("tasks".to_owned(), json!({"max_open_children": limit}));
    app.save_config(&config)?;
    Ok(app)
}

fn create_body(title: &str, parent: Option<&str>) -> serde_json::Value {
    let mut body = json!({
        "title": title, "description": "", "acceptance_criteria": ["it works"],
        "priority": "medium", "status": "backlog", "tags": [], "files": [],
        "projects": [], "repositories": [], "depends_on": [], "auto_execute": false,
    });
    if let Some(parent) = parent {
        body["parent"] = json!(parent);
    }
    body
}

fn create(app: &mut App, title: &str, parent: Option<&str>) -> Result<(), String> {
    let context = human_context(None).map_err(|e| e.to_string())?;
    service::task_create_from(app, &context, None, &create_body(title, parent), true)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn a_create_past_the_limit_is_refused_and_lists_the_children() -> TestResult {
    let mut app = app("refused", 2)?;
    create(&mut app, "Milestone", None)?;
    create(&mut app, "First child", Some("WG-1"))?;
    create(&mut app, "Second child", Some("WG-1"))?;
    let refusal = create(&mut app, "Third child", Some("WG-1"))
        .err()
        .unwrap_or_default();
    assert!(refusal.contains("too_many_children"), "{refusal}");
    assert!(refusal.contains("WG-2"), "{refusal}");
    assert!(refusal.contains("WG-3"), "{refusal}");
    assert!(refusal.contains("acceptance criterion"), "{refusal}");
    // Tasks without a parent are never counted.
    create(&mut app, "Another milestone", None)?;
    Ok(())
}

#[test]
fn a_finished_child_frees_a_place() -> TestResult {
    let mut app = app("freed", 1)?;
    create(&mut app, "Milestone", None)?;
    create(&mut app, "Only child", Some("WG-1"))?;
    assert!(create(&mut app, "Next child", Some("WG-1")).is_err());
    let context = human_context(None)?;
    service::task_update(
        &mut app,
        &context,
        None,
        "WG-2",
        &json!({"status": "cancelled"}),
    )?;
    create(&mut app, "Next child", Some("WG-1"))?;
    Ok(())
}

#[test]
fn zero_turns_the_check_off() -> TestResult {
    let mut app = app("off", 0)?;
    create(&mut app, "Milestone", None)?;
    for n in 0..5 {
        create(&mut app, &format!("Child {n}"), Some("WG-1"))?;
    }
    Ok(())
}
