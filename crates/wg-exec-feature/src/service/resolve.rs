//! The reference resolvers of the exec feature (`core/services/resolve.py`): a task named by UUID
//! or key, and an attempt named by UUID. The attempt tools and the `attempt` command group share
//! them, so a reference resolves and fails the same way on both fronts.

use serde_json::Value;
use wg_model::entity::AgentAttempt;
use wg_model::{RowKey, Task, py_repr_str};
use wg_store::Session;
use wg_types::{ErrorCode, Id, TaskKey, WgError, parse_id};

/// `resolve.task`: by UUID, else by key, within `workspace` when one is given. A key held by
/// tasks of more than one workspace is ambiguous when no workspace is given; a task outside
/// `workspace` is not found.
pub fn find_task(
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
pub fn find_attempt(session: &mut Session<'_>, reference: &str) -> Result<AgentAttempt, WgError> {
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
