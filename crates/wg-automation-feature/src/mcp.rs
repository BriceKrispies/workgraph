//! The automation tools (`src/workgraph/mcp/server.py`, M64 and M65): `automation_list` and
//! `automation_trigger`.
//!
//! Each tool calls the [`crate::service`] entry point its CLI command and route call, under the
//! agent's identity, so the permission checks are the services' own.

use serde_json::{Map, Value, json};
use wg_mcpspec::options::Agent;
use wg_mcpspec::{structured_output, text_argument};
use wg_runtime::{App, ToolRun, agent_context};
use wg_types::WgError;

use crate::service;

/// The feature's tool descriptor: one [`call`] that answers the automation tool names.
pub const TOOLS: &[ToolRun] = &[call];

/// The number of recent runs `automation_list` shows (Python's `limit=20`).
const RECENT_RUNS: i64 = 20;

/// Answers tool `name` when it is one of the automation tools; `None` leaves it to the other
/// features.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    let answer = match name {
        "automation_list" => list(app, agent, arguments),
        "automation_trigger" => trigger(app, agent, arguments),
        _ => return None,
    };
    Some(answer)
}

/// `automation_list` (M64): `{"rules": [...], "recent_runs": [...]}`.
fn list(app: &mut App, agent: &Agent, a: &Value) -> Result<Value, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let workspace = text_argument(a, "workspace");
    let rules = service::list(app, &context, workspace)?;
    let runs = service::runs(app, &context, workspace, None, None, RECENT_RUNS)?;
    Ok(json!({
        "rules": structured_output(&rules)?,
        "recent_runs": structured_output(&runs)?,
    }))
}

/// `automation_trigger` (M65): triggers a manual rule, with the task as its input when given.
fn trigger(app: &mut App, agent: &Agent, a: &Value) -> Result<Value, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let mut inputs = Map::new();
    if let Some(task) = text_argument(a, "task") {
        inputs.insert("task".to_owned(), Value::String(task.to_owned()));
    }
    let dto = service::trigger(
        app,
        &context,
        text_argument(a, "workspace"),
        text_argument(a, "rule").unwrap_or_default(),
        &inputs,
    )?;
    structured_output(&dto)
}
