//! The attempt and run tools (IF-33: M47 to M54, the tools of `mcp/server.py` that concern
//! attempts and runs).
//!
//! Each call checks the permissions its Python method requires (`wg_iface::mcp::tool_permissions`),
//! then resolves its references as `resolve.py` does: a task by UUID or key, within the workspace
//! the call names or across all workspaces when it names none (an ambiguous key is refused), and
//! an attempt by UUID. A reference that names nothing is the same `not_found` error Python gives.
//!
//! Every tool here is answered by the same service the attempt and run commands use: the attempt
//! writes ([`crate::service::attempt_runs`], [`crate::service::conclude`]) and the run flow
//! ([`crate::service::run_flow`]). The git tools of IF-33 are in the git feature's `mcp`.

use serde_json::Value;
use wg_iface::mcp::options::Agent;
use wg_iface::mcp::outcome::text_argument;
use wg_iface::mcp::{require, structured, tool_permissions};
use wg_runtime::{App, ToolRun, agent_context};
use wg_store::AttemptPage;
use wg_types::{AttemptStatus, Id, WgError};
use wg_wire::dto::{attempt, page};

use crate::service::agents::LiveAgents;
use crate::service::attempt_runs::{self, CreateRequest, StartRequest as AttemptStartRequest};
use crate::service::conclude::{self, Live};
use crate::service::resolve::{find_attempt, find_task};
use crate::service::run_flow;
use crate::spec::runs::StartRequest;
use wg_types::pyval::{int_or, text, text_or};

/// The feature's tool descriptor: one [`call`].
pub const TOOLS: &[ToolRun] = &[call];

/// Runs tool `name` for `agent`, or returns `None` when the tool is not one of this feature's.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    if !matches!(
        name,
        "attempt_create"
            | "attempt_start"
            | "attempt_submit"
            | "attempt_get"
            | "attempt_list"
            | "run_start"
            | "run_get"
            | "run_cancel"
    ) {
        return None;
    }
    let needed = tool_permissions(name)?;
    let actor = format!("agent:{}", agent.name);
    if let Err(err) = require(&actor, &agent.permissions, needed) {
        return Some(Err(err));
    }
    Some(match name {
        "attempt_create" => attempt_create_tool(app, agent, arguments),
        "attempt_start" => attempt_start_tool(app, agent, arguments),
        "attempt_submit" => attempt_submit_tool(app, agent, arguments),
        "attempt_get" => attempt_get(app, arguments),
        "attempt_list" => attempt_list(app, arguments),
        "run_get" => run_get_tool(app, arguments),
        "run_start" => run_start_tool(app, agent, arguments),
        _ => run_cancel_tool(app, agent, arguments),
    })
}

/// `attempt_create`: registers an attempt for this agent's work on a claimed task
/// (`AttemptService.create`, provider = the agent's name).
fn attempt_create_tool(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Value, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let request = CreateRequest {
        workspace: text_argument(arguments, "workspace"),
        task: text(arguments.get("task")),
        provider: &agent.name,
        adapter: None,
        agent_id: None,
        model: text_argument(arguments, "model"),
        session_id: text_argument(arguments, "session_id"),
        parent_attempt: text_argument(arguments, "parent_attempt"),
        status: AttemptStatus::Running,
    };
    structured(&attempt_runs::attempt_create(app, &context, &request)?)
}

/// `attempt_start`: claims the task, records an attempt and prepares its worktrees
/// (`RunService.start_external`). The reply names the checkouts to work in.
fn attempt_start_tool(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Value, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let request = AttemptStartRequest {
        workspace: text_argument(arguments, "workspace"),
        task: text(arguments.get("task")),
        agent_name: Some(&agent.name),
        adapter: text_or(arguments.get("adapter"), "claude_code_subagent"),
        model: text_argument(arguments, "model"),
        isolation: text_argument(arguments, "isolation"),
    };
    structured(&attempt_runs::attempt_start(app, &context, &request)?)
}

/// `attempt_submit`: concludes an external attempt from the commits the agent made in its
/// checkouts (`RunService.submit_external`), with the live collaborators the CLI uses.
fn attempt_submit_tool(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Value, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let failed = text_or(arguments.get("status"), "succeeded") == "failed";
    let out = conclude::attempt_submit(
        app,
        &context,
        &mut Live,
        text(arguments.get("attempt")),
        failed,
        text_argument(arguments, "summary"),
        text_argument(arguments, "error"),
    )?;
    structured(&out)
}

/// `run_get`: the attempt, its job and its recent events (`RunService.get`).
fn run_get_tool(app: &mut App, arguments: &Value) -> Result<Value, WgError> {
    let reference = text(arguments.get("attempt_id"));
    let events = int_or(arguments.get("events"), 50);
    structured(&crate::service::runs::get(app, reference, events)?)
}

/// `run_start`: queues a managed run of a task (`RunService.start`).
fn run_start_tool(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Value, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let request = StartRequest {
        agent: text("agent"),
        model: text("model"),
        ..StartRequest::default()
    };
    let started = run_flow::run_start(
        app,
        &context,
        &mut LiveAgents,
        text("workspace").as_deref(),
        &text("task").unwrap_or_default(),
        &request,
    )?;
    structured(&started)
}

/// `run_cancel`: asks a queued or running attempt to stop (`RunService.cancel`).
fn run_cancel_tool(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Value, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let reference = text(arguments.get("attempt_id"));
    let reason = arguments
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("cancelled via MCP");
    let cancelled = run_flow::run_cancel(app, &context, reference, reason)?;
    structured(&cancelled)
}

/// `attempt_get`: the attempt as `AttemptOut`.
fn attempt_get(app: &mut App, arguments: &Value) -> Result<Value, WgError> {
    let reference = text(arguments.get("attempt_id"));
    let found = app
        .runtime
        .store
        .read(|session| find_attempt(session, reference))?;
    structured(&attempt::from_model(&found))
}

/// `attempt_list`: one page of attempts in the workspace, newest first.
fn attempt_list(app: &mut App, arguments: &Value) -> Result<Value, WgError> {
    let workspace = workspace_or_default(app, arguments)?;
    let limit = int_or(arguments.get("limit"), 50);
    let offset = int_or(arguments.get("offset"), 0);
    let provider = arguments.get("provider").and_then(Value::as_str);
    // An unknown status never matches a row, so a filter of only unknown names matches nothing,
    // while an empty filter does not narrow at all.
    let wanted: Vec<&str> = arguments
        .get("statuses")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let statuses: Vec<AttemptStatus> = wanted
        .iter()
        .filter_map(|status| AttemptStatus::parse(status))
        .collect();
    let matches_nothing = !wanted.is_empty() && statuses.is_empty();
    let task_id = match arguments.get("task").and_then(Value::as_str) {
        Some(reference) => Some(
            app.runtime
                .store
                .read(|session| find_task(session, Some(workspace), reference))?
                .id,
        ),
        None => None,
    };
    let page = if matches_nothing {
        AttemptPage {
            items: Vec::new(),
            total: 0,
            limit,
            offset,
            next_offset: None,
        }
    } else {
        app.runtime.store.read(|session| {
            session.attempts_page(workspace, task_id, &statuses, provider, limit, offset)
        })?
    };
    let items: Vec<_> = page.items.iter().map(attempt::from_model).collect();
    structured(&page::from_model(
        items,
        page.total,
        page.limit,
        page.offset,
        page.next_offset,
    ))
}

/// The workspace the arguments name, else the default one (the tools that list a workspace).
fn workspace_or_default(app: &mut App, arguments: &Value) -> Result<Id, WgError> {
    match arguments.get("workspace").and_then(Value::as_str) {
        Some(reference) => app.workspace_id(reference),
        None => app.default_workspace_id(),
    }
}

/// The task history resource (`workgraph://workspaces/{workspace}/tasks/{task}/history`): the
/// task's events and its first page of attempts, as `{"events": [...], "attempts": {...}}`.
pub const RESOURCES: &[wg_runtime::ResourceRun] = &[resource];

/// Answers the history resource; `None` leaves the URI to the other features.
fn resource(app: &mut App, agent: &Agent, uri: &str) -> Option<Result<String, WgError>> {
    let rest = uri.strip_prefix("workgraph://workspaces/")?;
    let (workspace, tail) = rest.split_once('/')?;
    let task = tail.strip_prefix("tasks/")?.strip_suffix("/history")?;
    if task.is_empty() || task.contains('/') {
        return None;
    }
    Some(history_text(app, agent, workspace, task))
}

/// `HistoryService.task_history` and `AttemptService.list` (first 50, newest first) for the
/// resource, dumped as Python's `json.dumps` does.
fn history_text(
    app: &mut App,
    agent: &Agent,
    workspace: &str,
    task: &str,
) -> Result<String, WgError> {
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let events =
        wg_tasks_feature::service::task_history(app, &context, Some(workspace), task, &[])?;
    let attempts = crate::service::attempts::attempt_list(
        app,
        &context,
        &crate::service::attempts::ListQuery {
            workspace: Some(workspace),
            task: Some(task),
            statuses: &[],
            provider: None,
            limit: 50,
            offset: 0,
        },
    )?;
    let dto = wg_json::dict(vec![("events", events), ("attempts", attempts)]);
    wg_json::resource_json(&dto).map_err(|err| WgError::workgraph(err.to_string()))
}
