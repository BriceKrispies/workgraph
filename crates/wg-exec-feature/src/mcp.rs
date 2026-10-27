//! The attempt and run tools (IF-33: M47 to M54, the tools of `mcp/server.py` that concern
//! attempts and runs).
//!
//! Each call checks the permissions its Python method requires (`wg_mcpspec::tool_permissions`),
//! then resolves its references as `resolve.py` does: a task by UUID or key, within the workspace
//! the call names or across all workspaces when it names none (an ambiguous key is refused), and
//! an attempt by UUID. A reference that names nothing is the same `not_found` error Python gives.
//!
//! `attempt_get`, `attempt_list`, `run_get`, `run_start` and `run_cancel` are answered from the
//! store and the run flow. The other tools stop after their references resolve: their success
//! paths belong to the attempt group, which is not ported yet, so they answer with a
//! `workgraph_error` that names the tool. The git tools of the same family are answered by
//! `wg-mcp` until the git feature moves.

use serde_json::Value;
use wg_mcpspec::options::Agent;
use wg_mcpspec::{require, structured, tool_permissions};
use wg_model::entity::AgentAttempt;
use wg_model::{RowKey, Task, py_repr_str};
use wg_runtime::{App, ToolRun, agent_context};
use wg_store::{AttemptPage, Session};
use wg_types::{AttemptStatus, ErrorCode, Id, TaskKey, WgError, parse_id};
use wg_wire::dto::{attempt, page};

use crate::service::agents::LiveAgents;
use crate::service::run_flow;
use crate::spec::runs::StartRequest;

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
        "attempt_get" => attempt_get(app, arguments),
        "attempt_list" => attempt_list(app, arguments),
        "run_get" => run_get_tool(app, arguments),
        "run_start" => run_start_tool(app, agent, arguments),
        "run_cancel" => run_cancel_tool(app, agent, arguments),
        _ => unwired(app, name, arguments),
    })
}

/// `run_get`: the attempt, its job and its recent events (`RunService.get`).
fn run_get_tool(app: &mut App, arguments: &Value) -> Result<Value, WgError> {
    let reference = arguments
        .get("attempt_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let events = arguments
        .get("events")
        .and_then(Value::as_i64)
        .unwrap_or(50);
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
    let reference = arguments
        .get("attempt_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let reason = arguments
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("cancelled via MCP");
    let cancelled = run_flow::run_cancel(app, &context, reference, reason)?;
    structured(&cancelled)
}

/// `attempt_get`: the attempt as `AttemptOut`.
fn attempt_get(app: &mut App, arguments: &Value) -> Result<Value, WgError> {
    let reference = arguments
        .get("attempt_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let found = app
        .runtime
        .store
        .read(|session| find_attempt(session, reference))?;
    structured(&attempt::from_model(&found))
}

/// `attempt_list`: one page of attempts in the workspace, newest first.
fn attempt_list(app: &mut App, arguments: &Value) -> Result<Value, WgError> {
    let workspace = workspace_or_default(app, arguments)?;
    let limit = arguments.get("limit").and_then(Value::as_i64).unwrap_or(50);
    let offset = arguments.get("offset").and_then(Value::as_i64).unwrap_or(0);
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

/// The tools whose references resolve here and whose success path is not wired yet.
fn unwired(app: &mut App, name: &str, arguments: &Value) -> Result<Value, WgError> {
    if matches!(name, "attempt_submit") {
        let reference = arguments
            .get("attempt")
            .or_else(|| arguments.get("attempt_id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        app.runtime
            .store
            .read(|session| find_attempt(session, reference))?;
    } else {
        let workspace = workspace_arg(app, arguments)?;
        let reference = arguments
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or_default();
        app.runtime
            .store
            .read(|session| find_task(session, workspace, reference))?;
    }
    Err(WgError::new(
        ErrorCode::WorkgraphError,
        format!("tool {name} is not wired yet"),
    ))
}

/// The workspace the arguments name, else the default one (the tools that list a workspace).
fn workspace_or_default(app: &mut App, arguments: &Value) -> Result<Id, WgError> {
    match arguments.get("workspace").and_then(Value::as_str) {
        Some(reference) => app.workspace_id(reference),
        None => app.default_workspace_id(),
    }
}

/// The workspace the arguments name, if any. Without one a task reference is resolved across
/// every workspace, as `resolve.task(session, None, ref)` does.
fn workspace_arg(app: &mut App, arguments: &Value) -> Result<Option<Id>, WgError> {
    match arguments.get("workspace").and_then(Value::as_str) {
        Some(reference) => app.workspace_id(reference).map(Some),
        None => Ok(None),
    }
}

/// `resolve.task`: by UUID, else by key, within `workspace` when one is given. A key held by
/// tasks of more than one workspace is ambiguous when no workspace is given; a task outside
/// `workspace` is not found.
fn find_task(
    session: &mut Session<'_>,
    workspace: Option<Id>,
    reference: &str,
) -> Result<Task, WgError> {
    if reference.is_empty() {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "task reference must not be empty",
        ));
    }
    let by_id = match parse_id(reference) {
        Ok(id) => session.fetch::<Task>(&RowKey::Task(id))?,
        Err(_) => None,
    };
    let found = match by_id {
        Some(task) => Some(task),
        None => match TaskKey::parse(&reference.trim().to_uppercase()) {
            Ok(key) => match workspace {
                Some(ws) => session.task_by_key(ws, &key)?,
                None => {
                    // At most one match is needed to tell "one" from "ambiguous".
                    let mut matches = session.tasks_by_key(&key)?;
                    if matches.len() > 1 {
                        return Err(WgError::new(
                            ErrorCode::ValidationFailed,
                            format!(
                                "task key {} is ambiguous across workspaces; pass a workspace",
                                py_repr_str(reference)
                            ),
                        ));
                    }
                    matches.pop()
                }
            },
            Err(_) => None,
        },
    };
    found
        .filter(|task| workspace.is_none_or(|ws| task.workspace_id == ws))
        .ok_or_else(|| {
            WgError::new(
                ErrorCode::NotFound,
                format!("task {} not found", py_repr_str(reference)),
            )
            .with_detail("ref", Value::String(reference.to_owned()))
        })
}

/// `resolve.attempt`: by UUID.
fn find_attempt(session: &mut Session<'_>, reference: &str) -> Result<AgentAttempt, WgError> {
    let found = match parse_id(reference) {
        Ok(id) => session.attempt_by_id(id)?,
        Err(_) => None,
    };
    found.ok_or_else(|| {
        WgError::new(
            ErrorCode::NotFound,
            format!("attempt {} not found", py_repr_str(reference)),
        )
        .with_detail("ref", Value::String(reference.to_owned()))
    })
}
