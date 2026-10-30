//! The git tools of IF-33 (`git_get_changes`, `git_get_task_diff`, M55 and M56).
//!
//! Each call checks the permission its Python method requires (`wg_iface::mcp::tool_permissions`),
//! then answers with the same read as the `git changes` and `git diff` commands ([`crate::service`]).
//! The task reference is resolved there as `resolve.py` does: by UUID or key, within the workspace
//! the call names or across all workspaces when it names none (an ambiguous key is refused), and a
//! reference that names nothing is the same `not_found` error Python gives.

use serde_json::Value;
use wg_iface::mcp::options::Agent;
use wg_iface::mcp::outcome::text_argument;
use wg_iface::mcp::{require, structured, tool_permissions};
use wg_runtime::{App, ToolRun, agent_context};
use wg_types::WgError;
use wg_types::pyval::{int_or, text};

use crate::service;

/// The feature's tool descriptor: one [`call`].
pub const TOOLS: &[ToolRun] = &[call];

/// Runs tool `name` for `agent`, or returns `None` when the tool is not one of the git tools.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    if !matches!(name, "git_get_changes" | "git_get_task_diff") {
        return None;
    }
    let needed = tool_permissions(name)?;
    let actor = format!("agent:{}", agent.name);
    if let Err(err) = require(&actor, &agent.permissions, needed) {
        return Some(Err(err));
    }
    Some(match name {
        "git_get_changes" => git_get_changes(app, agent, arguments),
        _ => git_get_task_diff(app, agent, arguments),
    })
}

/// `git_get_changes`: the commits linked to the task, its churn and its change surfaces
/// (`GitService.task_changes`).
fn git_get_changes(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Value, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let min_confidence = arguments
        .get("min_confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.5);
    let out = service::git_changes(
        app,
        &context,
        text_argument(arguments, "workspace"),
        text(arguments.get("task")),
        min_confidence,
    )?;
    structured(&out)
}

/// `git_get_task_diff`: the full diff of the task read live from git, cut to `max_bytes`
/// (`GitService.task_diff`).
fn git_get_task_diff(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Value, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let max_bytes = usize::try_from(int_or(arguments.get("max_bytes"), 200_000)).unwrap_or(0);
    let out = service::git_diff(
        app,
        &context,
        text_argument(arguments, "workspace"),
        text(arguments.get("task")),
        text_argument(arguments, "repository"),
        max_bytes,
    )?;
    structured(&out)
}
