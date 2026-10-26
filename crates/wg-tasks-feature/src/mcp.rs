//! The task tools (`mcp/server.py`). The tools answered here are added one by one as their bodies
//! are ported: `task_contract_check` (M28) is the first. The others reach the generic `wg-mcp`
//! handling until they land. The tools are listed in `wg-mcpspec`'s catalog.

use crate::service::contract::task_contract_check;
use serde_json::Value;
use wg_mcpspec::options::Agent;
use wg_mcpspec::structured;
use wg_runtime::{App, ToolRun, agent_context};
use wg_types::WgError;

/// The feature's tool descriptor: the calls this feature answers.
pub const TOOLS: &[ToolRun] = &[call];

/// Runs tool `name` for `agent`, or returns `None` when the tool is not a task tool.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    match name {
        "task_contract_check" => Some(contract_check(app, agent, arguments)),
        _ => None,
    }
}

/// `task_contract_check` (M28): the arguments arrive validated with the default target filled in.
fn contract_check(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Value, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let reference = arguments
        .get("task")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let model = arguments
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let target = arguments
        .get("target")
        .and_then(Value::as_f64)
        .unwrap_or(0.8);
    let workspace = arguments.get("workspace").and_then(Value::as_str);
    structured(&task_contract_check(
        app, &context, workspace, reference, model, target,
    )?)
}
