//! The attempt, run and git tools (IF-33: M47 to M56).
//!
//! Each call checks the permissions its Python method requires (`wg_mcpspec::tool_permissions`),
//! then resolves its references as `resolve.py` does: a task by UUID or key within the workspace,
//! an attempt by UUID. A reference that names nothing is the same `not_found` error Python gives.
//!
//! `attempt_get` and `attempt_list` are answered from the store and the `wg-wire` DTOs. The other
//! tools stop after their references resolve: their success paths need the run flow (queued jobs,
//! adapters, worktree preparation) and the git runner, and those are not wired yet, so they answer
//! with a `workgraph_error` that names the tool.

use serde_json::Value;
use wg_mcpspec::{require, structured, tool_permissions};
use wg_model::entity::AgentAttempt;
use wg_model::{RowKey, Task, py_repr_str};
use wg_runtime::App;
use wg_store::{AttemptPage, Session};
use wg_types::{AttemptStatus, ErrorCode, Id, TaskKey, WgError, parse_id};
use wg_wire::dto::{attempt, page};

use crate::backend::Agent;

/// Runs tool `name` for `agent`, or returns `None` when the tool is not one of this group.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    let needed = tool_permissions(name)?;
    let actor = format!("agent:{}", agent.name);
    if let Err(err) = require(&actor, &agent.permissions, needed) {
        return Some(Err(err));
    }
    Some(match name {
        "attempt_get" => attempt_get(app, arguments),
        "attempt_list" => attempt_list(app, arguments),
        _ => unwired(app, name, arguments),
    })
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
    let workspace = workspace_of(app, arguments)?;
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
                .read(|session| find_task(session, workspace, reference))?
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
    if matches!(name, "attempt_submit" | "run_get" | "run_cancel") {
        let reference = arguments
            .get("attempt")
            .or_else(|| arguments.get("attempt_id"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        app.runtime
            .store
            .read(|session| find_attempt(session, reference))?;
    } else {
        let workspace = workspace_of(app, arguments)?;
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

/// The workspace the arguments name, else the default one.
fn workspace_of(app: &mut App, arguments: &Value) -> Result<Id, WgError> {
    match arguments.get("workspace").and_then(Value::as_str) {
        Some(reference) => app.workspace_id(reference),
        None => app.default_workspace_id(),
    }
}

/// `resolve.task`: by UUID, else by key within `workspace`; a task of another workspace is not
/// found.
fn find_task(session: &mut Session<'_>, workspace: Id, reference: &str) -> Result<Task, WgError> {
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
            Ok(key) => session.task_by_key(workspace, &key)?,
            Err(_) => None,
        },
    };
    found
        .filter(|task| task.workspace_id == workspace)
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
