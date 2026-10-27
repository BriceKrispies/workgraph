//! The git tools of IF-33 (`git_get_changes`, `git_get_task_diff`).
//!
//! Each call checks the permissions its Python method requires (`wg_mcpspec::tool_permissions`),
//! then resolves its task reference as `resolve.py` does: by UUID or key, within the workspace the
//! call names or across all workspaces when it names none (an ambiguous key is refused). A
//! reference that names nothing is the same `not_found` error Python gives. The success path needs
//! the git runner, which is not wired yet, so the call answers with a `workgraph_error` that names
//! the tool. The attempt and run tools of the same group are answered by the exec feature.

use serde_json::Value;
use wg_mcpspec::{require, tool_permissions};
use wg_model::entity::Task;
use wg_model::{RowKey, py_repr_str};
use wg_runtime::App;
use wg_store::Session;
use wg_types::{Id, TaskKey, WgError, parse_id};

use crate::backend::Agent;
use wg_types::pyval::text;

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
    Some(unwired(app, name, arguments))
}

/// The git tools: their references resolve here and their success path is not wired yet.
fn unwired(app: &mut App, name: &str, arguments: &Value) -> Result<Value, WgError> {
    let workspace = workspace_arg(app, arguments)?;
    let reference = text(arguments.get("task"));
    app.runtime
        .store
        .read(|session| find_task(session, workspace, reference))?;
    Err(WgError::workgraph(format!("tool {name} is not wired yet")))
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
        return Err(WgError::validation("task reference must not be empty"));
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
                        return Err(WgError::validation(format!(
                            "task key {} is ambiguous across workspaces; pass a workspace",
                            py_repr_str(reference)
                        )));
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
            WgError::not_found(format!("task {} not found", py_repr_str(reference)))
                .detail("ref", Value::String(reference.to_owned()))
        })
}
