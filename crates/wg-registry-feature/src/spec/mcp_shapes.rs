//! The MCP output shapes of the workspace, project and repository tools (CF-03).
//!
//! These are not DTOs and are not listed in [`super::implemented`]: a list tool returns a dict
//! holding the list under one key, while the get tools return the DTO itself.

use wg_json::{PyStr, Value};

fn list_under(key: &str, items: Vec<Value>) -> Value {
    Value::Dict(vec![(PyStr::from(key), Value::List(items))])
}

/// `workspace_list`: `{"workspaces": [WorkspaceOut, ...]}`.
pub fn workspaces(items: Vec<Value>) -> Value {
    list_under("workspaces", items)
}

/// `project_list`: `{"projects": [ProjectOut, ...]}`.
pub fn projects(items: Vec<Value>) -> Value {
    list_under("projects", items)
}

/// `repository_list`: `{"repositories": [RepositoryOut, ...]}`.
pub fn repositories(items: Vec<Value>) -> Value {
    list_under("repositories", items)
}
