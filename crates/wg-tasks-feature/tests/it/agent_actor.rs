//! `task claim --agent x` then `task result|complete|progress --agent x` succeed as one actor; a
//! report by any other actor is refused with the holder and the remedy named.

use std::error::Error;
use std::path::PathBuf;

use serde_json::{Value, json};
use wg_registry_feature::service::registry::workspace_create;
use wg_runtime::{App, human_context};
use wg_tasks_feature::cli::{GROUP, agent_actor};
use wg_tasks_feature::service;

type TestResult = Result<(), Box<dyn Error>>;

/// A fresh home for one test, under cargo's per-target temporary directory.
fn home(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("agent_actor")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// An app over a fresh home with one default workspace, `WG` keys.
fn app(name: &str) -> Result<App, Box<dyn Error>> {
    let mut app = App::open(Some(&home(name)?))?;
    let human = human_context(None)?;
    let body = json!({"slug": "main", "name": "Main", "description": "", "key_prefix": "WG"});
    workspace_create(&mut app, &human, &body, true)?;
    Ok(app)
}

/// Creates a ready task with one acceptance criterion, so it can be claimed at once. In a fresh
/// home the first task is `WG-1`.
fn ready_task(app: &mut App, human: &wg_runtime::RequestContext) -> Result<String, Box<dyn Error>> {
    let body = json!({
        "title": "Agent identity",
        "description": "",
        "acceptance_criteria": ["the report is recorded"],
        "priority": "medium",
        "status": "ready",
        "tags": [],
        "files": [],
        "projects": [],
        "repositories": [],
        "depends_on": [],
        "auto_execute": false,
    });
    service::task_create(app, human, None, &body)?;
    Ok("WG-1".to_owned())
}

fn claim_body(agent: Option<&str>) -> Value {
    json!({"agent": agent, "lease_minutes": 120})
}

fn result_body() -> Value {
    json!({"outcome": "succeeded", "summary": "done", "attempt_id": null})
}

#[test]
fn a_claim_and_its_report_with_the_same_agent_name_succeed() -> TestResult {
    let mut app = app("same_agent")?;
    let human = human_context(None)?;
    let key = ready_task(&mut app, &human)?;

    // `task claim --agent x`: the human context claims as agent:x.
    service::task_claim(&mut app, &human, None, &key, &claim_body(Some("x")))?;
    // `task result --agent x`: the report runs as agent:x, the holder.
    let actor = agent_actor(Some("x"), &human)?;
    assert_eq!(actor.actor.to_string(), "agent:x");
    service::task_result(&mut app, &actor, None, &key, &result_body())?;
    Ok(())
}

#[test]
fn a_report_by_another_actor_is_refused_naming_the_holder_and_the_remedy() -> TestResult {
    let mut app = app("holder_refused")?;
    let human = human_context(None)?;
    let key = ready_task(&mut app, &human)?;

    // A plain `task claim` by the human records human:<user> as the holder.
    service::task_claim(&mut app, &human, None, &key, &claim_body(None))?;
    let Err(refused) = service::task_result(
        &mut app,
        &agent_actor(Some("x"), &human)?,
        None,
        &key,
        &result_body(),
    ) else {
        return Err("an agent that did not claim must be refused".into());
    };
    assert!(
        refused.message.contains("claimed by human:"),
        "{}",
        refused.message
    );
    assert!(
        refused.message.contains("--agent <name>"),
        "{}",
        refused.message
    );
    assert!(
        refused.message.contains("WORKGRAPH_AGENT"),
        "{}",
        refused.message
    );

    // The holder itself (no --agent) reports fine.
    service::task_result(
        &mut app,
        &agent_actor(None, &human)?,
        None,
        &key,
        &result_body(),
    )?;
    Ok(())
}

#[test]
fn the_report_commands_and_claim_read_the_agent_variable() -> TestResult {
    // clap reads `WORKGRAPH_AGENT` as the default of `--agent` on every mutating task command the
    // contract names it for; the variable is set through the contract, not the process
    // environment, so the test does not race other tests.
    let task = (GROUP.command)();
    for command in ["claim", "result", "complete", "progress"] {
        let sub = task
            .find_subcommand(command)
            .ok_or_else(|| format!("task {command} is missing"))?;
        let agent = sub
            .get_arguments()
            .find(|arg| arg.get_long() == Some("agent"))
            .ok_or_else(|| format!("task {command} has no --agent"))?;
        assert_eq!(
            agent.get_env().and_then(|var| var.to_str()),
            Some("WORKGRAPH_AGENT"),
            "task {command}"
        );
    }
    Ok(())
}

#[test]
fn an_empty_agent_name_is_no_agent() -> TestResult {
    let human = human_context(None)?;
    assert_eq!(agent_actor(Some(""), &human)?.actor, human.actor);
    assert_eq!(agent_actor(None, &human)?.actor, human.actor);
    Ok(())
}
