//! The task rows a registry rename rewrites (`RegistryService.rename` updates each task through
//! `TaskService.update(..., enrich=False)`). The update decision is `wg_tasks::edit`; this shell
//! reads and applies it in place, because the registry is still a base module. FC-05 moves it
//! into the registry's feature crate, which may call the tasks feature.

use serde_json::Value as Json;
use wg_model::slice::QueueSlice;
use wg_registry::resolve;
use wg_runtimespec::RequestContext;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::edit::{Enrichment, read_update, update_task, validate_update};
use wg_types::WgError;

use crate::app::App;
use crate::scope::index_written;

impl App {
    /// `TaskService.update(..., enrich=False)`: a text change that queues no reclassification or
    /// re-embedding (the words changed, not the task).
    pub(crate) fn task_update_plain(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<wg_json::Value, WgError> {
        let dump = validate_update(body)?;
        let given = body.as_object().cloned().unwrap_or_default();
        let read = read_update(&dump, &given)?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
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
}
