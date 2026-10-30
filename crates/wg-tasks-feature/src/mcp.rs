//! The task tools (`src/workgraph/mcp/server.py`, M26 to M46): `task_create`, `task_get`,
//! `task_list`, `task_update`, `task_claim`, `task_release`, `task_complete`, `task_get_ready`,
//! `task_promote_ready`, the dependency, relation, comment, history, attempt, repository, commit,
//! progress and result tools.
//!
//! Each tool turns its validated arguments into the body its service reads ([`crate::spec::tool_args`])
//! and calls the feature's [`crate::service`] entry point under the agent's identity, so the
//! permission checks are the services' own, as in Python. `task_search` (M29) is not
//! answered here yet: its service is not ported (the hybrid search ranking), so that name reaches
//! the generic "not wired" answer.

use crate::service::contract::task_contract_check;
use serde_json::Value;
use wg_json::Value as Dto;
use wg_mcpspec::options::Agent;
use wg_mcpspec::{structured_output, text_argument};
use wg_runtime::{App, ToolRun, agent_context};
use wg_types::WgError;

use crate::service;
use crate::spec::query::first_shown;
use crate::spec::tool_args as args;
use wg_types::pyval::text;

/// The feature's tool descriptor: one [`call`] that answers the task tool names.
pub const TOOLS: &[ToolRun] = &[call];

/// Answers tool `name` when it is one of the task tools; `None` leaves it to the other features.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    let answer = match name {
        "task_create" => create(app, agent, arguments),
        "task_get" => get(app, agent, arguments),
        "task_list" => list(app, agent, arguments),
        "task_update" => update(app, agent, arguments),
        "task_claim" => claim(app, agent, arguments),
        "task_release" => release(app, agent, arguments),
        "task_complete" => complete(app, agent, arguments),
        "task_get_ready" => get_ready(app, agent, arguments),
        "task_promote_ready" => promote_ready(app, agent, arguments),
        "task_add_dependency" => add_dependency(app, agent, arguments),
        "task_remove_dependency" => remove_dependency(app, agent, arguments),
        "task_add_relation" => add_relation(app, agent, arguments),
        "task_add_comment" => add_comment(app, agent, arguments),
        "task_get_history" => get_history(app, agent, arguments),
        "task_get_attempts" => get_attempts(app, agent, arguments),
        "task_link_repository" => link_repository(app, agent, arguments),
        "task_link_commit" => link_commit(app, agent, arguments),
        "task_report_progress" => report_progress(app, agent, arguments),
        "task_report_result" => report_result(app, agent, arguments),
        "task_contract_check" => contract_check(app, agent, arguments),
        _ => return None,
    };
    Some(answer.and_then(|dto| structured_output(&dto)))
}

/// `TaskService.create` for `task_create` (M26).
fn create(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_create(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::create_body(a),
    )
}

/// `TaskService.detail` for `task_get` (M27): the task, or the error of its reference.
fn get(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let reference = args::text(a, "task");
    first_shown(service::task_show(
        app,
        &context,
        text_argument(a, "workspace"),
        &[reference],
    )?)
}

/// `TaskService.list` for `task_list` (M30).
fn list(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_list(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::filter_body(a),
    )
}

/// `TaskService.update` for `task_update` (M31).
fn update(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_update(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::update_body(a),
    )
}

/// `TaskService.claim` for `task_claim` (M32).
fn claim(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_claim(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::claim_body(a),
    )
}

/// `TaskService.release` for `task_release` (M33).
fn release(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_release(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::release_body(a),
    )
}

/// `TaskService.complete` for `task_complete` (M34): awaiting review, never accepted.
fn complete(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_complete(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::complete_body(a),
    )
}

/// `TaskService.get_ready` for `task_get_ready` (M35), read for this agent, under `{"tasks": …}`.
fn get_ready(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let tasks = service::task_ready(
        app,
        &context,
        text_argument(a, "workspace"),
        text_argument(a, "project"),
        text_argument(a, "repository"),
        Some(agent.name.as_str()),
        args::number(a, "limit", 20),
    )?;
    Ok(Dto::Dict(vec![("tasks".into(), tasks)]))
}

/// `TaskService.promote_ready_report` with `dry_run`, for `task_promote_ready` (M36).
fn promote_ready(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let scope = args::promote_scope(a)?;
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_promote_ready(
        app,
        &context,
        text_argument(a, "workspace"),
        &scope.tags,
        scope.project.as_deref(),
        scope.dry_run,
    )
}

/// `TaskService.add_dependency` for `task_add_dependency` (M37).
fn add_dependency(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_add_dependency(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::dependency_body(a),
    )
}

/// `TaskService.remove_dependency` for `task_remove_dependency` (M38).
fn remove_dependency(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_remove_dependency(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::text(a, "depends_on"),
    )
}

/// `TaskService.add_relation` for `task_add_relation` (M39).
fn add_relation(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_add_relation(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::relation_body(a),
    )
}

/// `TaskService.add_comment` for `task_add_comment` (M40).
fn add_comment(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_comment(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::comment_body(a),
    )
}

/// `HistoryService.task_history` for `task_get_history` (M41), under `{"events": …}`.
fn get_history(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let events = service::task_history_limited(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &[],
        args::number(a, "limit", 500),
    )?;
    Ok(Dto::Dict(vec![("events".into(), events)]))
}

/// `AttemptService.list` for one task, for `task_get_attempts` (M42).
fn get_attempts(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_attempts(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        args::number(a, "limit", 50),
        args::number(a, "offset", 0),
    )
}

/// `TaskService.link_repository` for `task_link_repository` (M43).
fn link_repository(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_link_repository(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::repo_body(a),
    )
}

/// `GitService.link_commit` for `task_link_commit` (M44), under `{"links": …}`.
fn link_commit(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let links = wg_git_feature::service::git_link(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::text(a, "sha"),
        text_argument(a, "repository"),
        &args::text(a, "note"),
    )?;
    Ok(Dto::Dict(vec![("links".into(), links)]))
}

/// `TaskService.report_progress` for `task_report_progress` (M45).
fn report_progress(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_progress(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::progress_body(a),
    )
}

/// `TaskService.report_result` for `task_report_result` (M46).
fn report_result(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::task_result(
        app,
        &context,
        text_argument(a, "workspace"),
        &args::text(a, "task"),
        &args::result_body(a),
    )
}

/// `task_contract_check` (M28): the arguments arrive validated with the default target filled in.
fn contract_check(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let reference = text(a.get("task"));
    let model = text(a.get("model"));
    let target = a.get("target").and_then(Value::as_f64).unwrap_or(0.8);
    let workspace = a.get("workspace").and_then(Value::as_str);
    task_contract_check(app, &context, workspace, reference, model, target)
}

/// The task resource (`workgraph://workspaces/{workspace}/tasks/{task}`): the task's detail, as
/// `task_get` shows it.
pub const RESOURCES: &[wg_runtime::ResourceRun] = &[resource];

/// Answers the task resource; `None` leaves the URI to the other features.
fn resource(app: &mut App, agent: &Agent, uri: &str) -> Option<Result<String, WgError>> {
    let rest = uri.strip_prefix("workgraph://workspaces/")?;
    let (workspace, tail) = rest.split_once('/')?;
    let task = tail.strip_prefix("tasks/")?;
    if task.contains('/') {
        return None;
    }
    Some(task_text(app, agent, workspace, task))
}

/// `TaskService.detail` for the resource, dumped as Python's `json.dumps` does.
fn task_text(app: &mut App, agent: &Agent, workspace: &str, task: &str) -> Result<String, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let dto = first_shown(service::task_show(
        app,
        &context,
        Some(workspace),
        &[task.to_owned()],
    )?)?;
    wg_json::log_json(&dto).map_err(|err| WgError::workgraph(err.to_string()))
}
