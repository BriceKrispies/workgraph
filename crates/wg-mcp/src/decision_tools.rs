//! The decision tools: `task_request_decision`, `decision_list` and `task_decisions`
//! (`mcp/server.py`).
//!
//! Each call runs the `App` entry point of `DecisionService` under the agent's identity
//! (`RequestContext.agent(name, permissions)`), which checks the permissions the Python service
//! requires, then wraps the document the way the tool returns it.

use serde_json::{Map, Value};
use wg_mcpspec::structured;
use wg_runtime::{App, agent_context};
use wg_runtimespec::decisions::{decisions, list_args, request_parts, text_param};
use wg_types::WgError;

use crate::backend::Agent;

/// Runs tool `name` for `agent`, or returns `None` when the tool is not a decision tool.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    let wanted = arguments.as_object().cloned().unwrap_or_default();
    let result = match name {
        "task_request_decision" => run(app, agent, &wanted, request),
        "decision_list" => run(app, agent, &wanted, list),
        "task_decisions" => run(app, agent, &wanted, for_task),
        _ => return None,
    };
    Some(result)
}

/// One tool body: the application, the agent's context, the workspace argument and the arguments.
type Body = fn(
    &mut App,
    &wg_runtime::RequestContext,
    Option<&str>,
    &Map<String, Value>,
) -> Result<wg_json::Value, WgError>;

/// Builds the agent's context, runs `body` and shapes its document as structured output.
fn run(
    app: &mut App,
    agent: &Agent,
    arguments: &Map<String, Value>,
    body: Body,
) -> Result<Value, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let workspace = arguments.get("workspace").and_then(Value::as_str);
    structured(&body(app, &context, workspace, arguments)?)
}

fn request(
    app: &mut App,
    context: &wg_runtime::RequestContext,
    workspace: Option<&str>,
    arguments: &Map<String, Value>,
) -> Result<wg_json::Value, WgError> {
    let (question, options) = request_parts(&Value::Object(arguments.clone()));
    app.decision_request(
        context,
        workspace,
        text_param(arguments, "task"),
        &question,
        &options,
    )
}

fn list(
    app: &mut App,
    context: &wg_runtime::RequestContext,
    workspace: Option<&str>,
    arguments: &Map<String, Value>,
) -> Result<wg_json::Value, WgError> {
    let (filter, limit) = list_args(arguments);
    app.decision_list(context, workspace, filter, limit)
        .map(decisions)
}

fn for_task(
    app: &mut App,
    context: &wg_runtime::RequestContext,
    workspace: Option<&str>,
    arguments: &Map<String, Value>,
) -> Result<wg_json::Value, WgError> {
    app.decision_for_task(context, workspace, text_param(arguments, "task"))
        .map(decisions)
}
