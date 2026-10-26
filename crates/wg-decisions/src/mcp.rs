//! The decision tools: `task_request_decision`, `decision_list` and `task_decisions`
//! (`mcp/server.py`).
//!
//! Each call runs the [`crate::service`] entry point of the decision under the agent's identity
//! (`RequestContext.agent(name, permissions)`), which checks the permissions the Python service
//! requires, then wraps the document the way the tool returns it. [`TABLE`] names the tools this
//! feature answers; [`TOOLS`] is the descriptor's table.

use serde_json::{Map, Value};
use wg_mcpspec::options::Agent;
use wg_mcpspec::structured;
use wg_runtime::{App, RequestContext, ToolRun, agent_context};
use wg_runtimespec::text_param;
use wg_types::WgError;

use crate::service;
use crate::spec::{decisions, list_args, request_parts};

/// One tool body: the application, the agent's context, the workspace argument and the arguments.
pub type Body = fn(
    &mut App,
    &RequestContext,
    Option<&str>,
    &Map<String, Value>,
) -> Result<wg_json::Value, WgError>;

/// The tools of the feature, by name, with the body each one runs.
pub const TABLE: &[(&str, Body)] = &[
    ("task_request_decision", request),
    ("decision_list", list),
    ("task_decisions", for_task),
];

/// The feature's tool descriptor: one [`call`].
pub const TOOLS: &[ToolRun] = &[call];

/// Runs tool `name` for `agent`, or returns `None` when the tool is not a decision tool.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    let (_, body) = TABLE.iter().find(|(tool, _)| *tool == name)?;
    let wanted = arguments.as_object().cloned().unwrap_or_default();
    Some(run(app, agent, &wanted, *body))
}

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
    context: &RequestContext,
    workspace: Option<&str>,
    arguments: &Map<String, Value>,
) -> Result<wg_json::Value, WgError> {
    let (question, options) = request_parts(&Value::Object(arguments.clone()));
    service::request(
        app,
        context,
        workspace,
        text_param(arguments, "task"),
        &question,
        &options,
    )
}

fn list(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    arguments: &Map<String, Value>,
) -> Result<wg_json::Value, WgError> {
    let (filter, limit) = list_args(arguments);
    service::list(app, context, workspace, filter, limit).map(decisions)
}

fn for_task(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    arguments: &Map<String, Value>,
) -> Result<wg_json::Value, WgError> {
    service::for_task(app, context, workspace, text_param(arguments, "task")).map(decisions)
}
