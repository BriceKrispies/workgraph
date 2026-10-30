//! The event entry points of the application: `events list` (the history) and `events verify` (the
//! replay of the task events against the task rows).
//!
//! Each one checks the read permission, resolves the workspace, reads the rows it needs in one
//! transaction and hands them to the pure functions of `wg-events` and [`crate::spec`]. These are
//! free functions over `&mut App`, the feature's Shell, reached from [`crate::cli`].

use wg_events::{VerifyReport, task_states, verify_task_projection};
use wg_json::Value;
use wg_model::StoredEvent;
use wg_rows::EventQuery;
use wg_runtime::{App, RequestContext};
use wg_types::{Permission, WgError};
use wg_wire::dto::event::from_stored;

use crate::spec::{self, ListArgs};

/// Checks the workspace-read permission against a fresh stamp, before anything is read.
fn require_read(app: &mut App, context: &RequestContext) -> Result<(), WgError> {
    app.runtime
        .stamps
        .stamp(context)?
        .require(&[Permission::WorkspaceRead])
}

/// `events list`: the events matching the arguments, as `EventOut` objects in query order. Without
/// a workspace the history covers every workspace.
pub fn list(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    args: &ListArgs,
) -> Result<Vec<Value>, WgError> {
    require_read(app, context)?;
    let workspace_id = match workspace {
        Some(reference) => Some(app.workspace_id(reference)?),
        None => None,
    };
    let Some(query) = spec::list_query(args, workspace_id) else {
        return Ok(Vec::new());
    };
    let records = app
        .runtime
        .store
        .read(|session| session.query_events(&query))?;
    Ok(records
        .into_iter()
        .map(|record| from_stored(&StoredEvent::from(record)))
        .collect())
}

/// `events verify`: replays the workspace's task events and compares the result with the task
/// rows (`history.verify_task_projection`). `workspace` names the workspace; without it the
/// default workspace is verified.
pub fn verify(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
) -> Result<VerifyReport, WgError> {
    require_read(app, context)?;
    let workspace_id = match workspace {
        Some(reference) => app.workspace_id(reference)?,
        None => app.default_workspace_id()?,
    };
    let (events, tasks) = app.runtime.store.read(|session| {
        let events = session.query_events(&EventQuery {
            workspace_id: Some(workspace_id),
            entity_type: Some("task".to_owned()),
            limit: 10_000_000,
            ..EventQuery::default()
        })?;
        let world = session.task_world(Some(workspace_id), &[])?;
        Ok((events, world.tasks))
    })?;
    verify_task_projection(
        events.into_iter().map(StoredEvent::from).collect(),
        &task_states(&tasks),
    )
    .map_err(|error| WgError::workgraph(error.to_string()))
}
