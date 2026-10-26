//! The task rows a registry rename rewrites: `RegistryService.rename` updates each task through
//! `TaskService.update(..., enrich=False)`. The update decision is `wg_tasks::edit`; this is the
//! shell that reads and applies it. The registry feature calls it for each task of a rename.

use serde_json::Value as Json;
use wg_json::Value;
use wg_model::slice::QueueSlice;
use wg_registry::resolve;
use wg_runtime::{App, RequestContext, index_written};
use wg_store::TaskWorld;
use wg_tasks::edit::{Enrichment, read_update, update_task, validate_update};
use wg_types::WgError;

/// `TaskService.update(..., enrich=False)`: a text change that queues no reclassification or
/// re-embedding (the words changed, not the task).
pub fn task_update_plain(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    body: &Json,
) -> Result<Value, WgError> {
    let dump = validate_update(body)?;
    let given = body.as_object().cloned().unwrap_or_default();
    let read = read_update(&dump, &given)?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| {
                let world = session.task_world(scope, &[])?;
                let queue = match resolve::task(&world.tasks, scope, reference) {
                    Ok(task) => session.task_jobs_slice(task.id)?,
                    Err(_) => QueueSlice::default(),
                };
                Ok((world, queue))
            },
            |(world, queue): &(TaskWorld, QueueSlice), stamp| {
                update_task(
                    world,
                    queue,
                    stamp,
                    scope,
                    reference,
                    read,
                    Enrichment::default(),
                )
            },
            index_written,
        )?
        .reply)
}
