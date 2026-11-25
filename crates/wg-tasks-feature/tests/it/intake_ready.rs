//! An agent's `task_create` with `ready` under `tasks.intake_proposed` lands ready when the
//! promote-ready rule would move it, and stays proposed with the reason otherwise. `task
//! promote-ready` reports each candidate it leaves out, with the reason.

use std::error::Error;
use std::path::PathBuf;

use serde_json::json;
use wg_json::{PyStr, Value};
use wg_registry_feature::service::registry::workspace_create;
use wg_runtime::{App, human_context};
use wg_tasks_feature::service;

type TestResult = Result<(), Box<dyn Error>>;

/// A fresh workspace `main` with `tasks.intake_proposed` on.
fn app(name: &str) -> Result<App, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("intake_ready")
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
    config.insert("tasks".to_owned(), json!({"intake_proposed": true}));
    app.save_config(&config)?;
    Ok(app)
}

fn create_body(title: &str, status: &str, tags: &[&str], depends_on: &[&str]) -> serde_json::Value {
    json!({
        "title": title, "description": "", "acceptance_criteria": ["it works"],
        "priority": "medium", "status": status, "tags": tags, "files": [],
        "projects": [], "repositories": [], "depends_on": depends_on, "auto_execute": false,
    })
}

/// The named member of a reply (a dict or a model), if it has one.
fn field<'a>(reply: &'a Value, name: &str) -> Option<&'a Value> {
    match reply {
        Value::Dict(entries) => entries
            .iter()
            .find(|(key, _)| *key == PyStr::from(name))
            .map(|(_, value)| value),
        Value::Model { fields, .. } => fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value),
        _ => None,
    }
}

fn text_of(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Str(text)) => text.to_text(),
        _ => None,
    }
}

/// The status of a created task's reply.
fn status(reply: &Value) -> Option<String> {
    text_of(field(reply, "status"))
}

/// The reason a promote-ready dry run gives for skipping `key`, if it skips it.
fn skip_reason(report: &Value, key: &str) -> Option<String> {
    match field(report, "skipped") {
        Some(Value::Dict(entries)) => text_of(
            entries
                .iter()
                .find(|(name, _)| *name == PyStr::from(key))
                .map(|(_, value)| value),
        ),
        _ => None,
    }
}

#[test]
fn an_agent_ready_create_lands_ready_when_promote_would_move_it() -> TestResult {
    let mut app = app("lands_ready")?;
    let context = human_context(None)?;
    let reply = service::task_create_from(
        &mut app,
        &context,
        None,
        &create_body("Plain", "ready", &[], &[]),
        true,
    )?;
    assert_eq!(status(&reply).as_deref(), Some("ready"));
    let warning = service::create_warning(&reply).unwrap_or_default();
    assert!(!warning.contains("stays proposed"), "{warning}");
    Ok(())
}

#[test]
fn an_agent_ready_create_with_an_unaccepted_dependency_stays_proposed() -> TestResult {
    let mut app = app("unaccepted_dependency")?;
    let human = human_context(None)?;
    service::task_create(
        &mut app,
        &human,
        None,
        &create_body("Base", "backlog", &[], &[]),
    )?;
    let reply = service::task_create_from(
        &mut app,
        &human,
        None,
        &create_body("Follow", "ready", &[], &["WG-1"]),
        true,
    )?;
    assert_eq!(status(&reply).as_deref(), Some("proposed"));
    let warning = service::create_warning(&reply).unwrap_or_default();
    assert!(warning.contains("stays proposed"), "{warning}");
    assert!(warning.contains("WG-1"), "{warning}");
    Ok(())
}

#[test]
fn an_agent_ready_create_tagged_parked_stays_proposed() -> TestResult {
    let mut app = app("parked")?;
    let human = human_context(None)?;
    let reply = service::task_create_from(
        &mut app,
        &human,
        None,
        &create_body("Held", "ready", &["parked"], &[]),
        true,
    )?;
    assert_eq!(status(&reply).as_deref(), Some("proposed"));
    let warning = service::create_warning(&reply).unwrap_or_default();
    assert!(warning.contains("parked"), "{warning}");
    Ok(())
}

#[test]
fn promote_ready_dry_run_names_each_skipped_candidate_and_why() -> TestResult {
    let mut app = app("dry_run_reasons")?;
    let human = human_context(None)?;
    // WG-1: a proposed task (an agent create without ready under intake).
    service::task_create_from(
        &mut app,
        &human,
        None,
        &create_body("Proposal", "backlog", &[], &[]),
        true,
    )?;
    // WG-2 has a child; WG-3 is that child and moves.
    service::task_create(
        &mut app,
        &human,
        None,
        &create_body("Parent", "backlog", &[], &[]),
    )?;
    let mut child = create_body("Child", "backlog", &[], &[]);
    child["parent"] = json!("WG-2");
    service::task_create(&mut app, &human, None, &child)?;
    // WG-4 is parked; WG-5 waits for the proposed WG-1.
    service::task_create(
        &mut app,
        &human,
        None,
        &create_body("Held", "backlog", &["parked"], &[]),
    )?;
    service::task_create(
        &mut app,
        &human,
        None,
        &create_body("Waits", "backlog", &[], &["WG-1"]),
    )?;

    let report = service::task_promote_ready(&mut app, &human, None, &[], None, true)?;
    let skipped = skip_reason(&report, "WG-1").unwrap_or_default();
    assert!(skipped.contains("proposed"), "{skipped}");
    assert!(
        skip_reason(&report, "WG-2")
            .unwrap_or_default()
            .contains("child")
    );
    assert!(
        skip_reason(&report, "WG-4")
            .unwrap_or_default()
            .contains("parked")
    );
    let waits = skip_reason(&report, "WG-5").unwrap_or_default();
    assert!(waits.contains("WG-1"), "{waits}");
    assert!(
        skip_reason(&report, "WG-3").is_none(),
        "WG-3 moves, so it is not skipped"
    );
    Ok(())
}
