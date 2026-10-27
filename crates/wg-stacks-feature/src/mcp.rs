//! The stack tools (`src/workgraph/mcp/server.py`, M09 to M16): `stack_get`, `stack_list`,
//! `stack_history`, `stack_create`, `stack_add_item`, `stack_remove_item`, `stack_integrate` and
//! `stack_verify`.
//!
//! Each tool calls its [`crate::service`] entry point under the agent's identity, so the permission
//! checks are the application's own, as in Python: the reads need `task:read`, which agents hold by
//! default, and the mutations need `stack:write`, which they do not. The arguments arrive validated
//! with their defaults filled in, so `include_inactive` is read as false when it is absent.
//! `stack_list` and `stack_history` wrap their lists as `{"stacks": …}` and `{"events": …}`, as the
//! Python tools do; the other tools answer the application's object.

use serde_json::Value;
use wg_json::Value as Dto;
use wg_mcpspec::options::Agent;
use wg_mcpspec::{structured_output, text_argument};
use wg_runtime::{App, ToolRun, agent_context};
use wg_types::WgError;

use crate::service;

/// The feature's tool descriptor: one [`call`] that answers the stack tool names.
pub const TOOLS: &[ToolRun] = &[call];

/// Answers tool `name` when it is one of the stack tools; `None` leaves it to the other features.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    let answer = match name {
        "stack_get" => get(app, agent, arguments),
        "stack_list" => list(app, agent, arguments),
        "stack_history" => history(app, agent, arguments),
        "stack_create" => create(app, agent, arguments),
        "stack_add_item" => add_item(app, agent, arguments),
        "stack_remove_item" => remove_item(app, agent, arguments),
        "stack_integrate" => integrate(app, agent, arguments),
        "stack_verify" => verify(app, agent, arguments),
        _ => return None,
    };
    Some(answer.and_then(|dto| structured_output(&dto)))
}

/// A required string argument; the catalog has already checked it is present.
fn text<'a>(a: &'a Value, key: &str) -> &'a str {
    a.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// `StackService.get` for `stack_get` (M09): the parent's most recent stack.
fn get(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::show(
        app,
        &context,
        text_argument(a, "workspace"),
        text(a, "parent"),
    )
}

/// `StackService.list` for `stack_list` (M10), under `{"stacks": …}`.
fn list(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let include_inactive = a
        .get("include_inactive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let stacks = service::list(
        app,
        &context,
        text_argument(a, "workspace"),
        include_inactive,
    )?;
    Ok(Dto::Dict(vec![("stacks".into(), stacks)]))
}

/// `StackService.history` for `stack_history` (M11), under `{"events": …}`.
fn history(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let events = service::history(
        app,
        &context,
        text_argument(a, "workspace"),
        text(a, "parent"),
    )?;
    Ok(Dto::Dict(vec![("events".into(), events)]))
}

/// `StackService.create` for `stack_create` (M12).
fn create(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::create(
        app,
        &context,
        text_argument(a, "workspace"),
        text(a, "parent"),
    )
}

/// `StackService.add_item` for `stack_add_item` (M13).
fn add_item(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::add(
        app,
        &context,
        text_argument(a, "workspace"),
        text(a, "parent"),
        text(a, "task"),
    )
}

/// `StackService.remove_item` for `stack_remove_item` (M14).
fn remove_item(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::remove(
        app,
        &context,
        text_argument(a, "workspace"),
        text(a, "parent"),
        text(a, "task"),
    )
}

/// `StackService.integrate` for `stack_integrate` (M15).
fn integrate(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::integrate(
        app,
        &context,
        text_argument(a, "workspace"),
        text(a, "parent"),
    )
}

/// `StackVerifier.request` for `stack_verify` (M16).
fn verify(app: &mut App, agent: &Agent, a: &Value) -> Result<Dto, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::verify(
        app,
        &context,
        text_argument(a, "workspace"),
        text(a, "parent"),
    )
}
