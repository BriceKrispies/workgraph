//! The workspace scope of an application call and the full-text index of the task rows a
//! decision wrote: the base helpers every feature uses (`workspace_or_default`, `require_read`).

use std::collections::BTreeSet;

use crate::spec::RequestContext;
use wg_model::{Row, Write};
use wg_store::Session;
use wg_types::{Id, Permission, WgError};

use crate::app::App;

pub fn index_written<R>(
    session: &mut Session<'_>,
    decision: &wg_model::Decision<R>,
) -> Result<(), WgError> {
    let written: BTreeSet<Id> = decision
        .writes
        .iter()
        .filter_map(|write| match write {
            Write::Insert(Row::Task(task))
            | Write::Update {
                row: Row::Task(task),
                ..
            } => Some(task.id),
            Write::Insert(Row::TaskComment(comment)) => Some(comment.task_id),
            _ => None,
        })
        .collect();
    written
        .into_iter()
        .try_for_each(|task_id| session.index_task(task_id))
}

impl App {
    /// `TaskService._workspace_id`: the named workspace, else the default one.
    pub fn workspace_or_default(&mut self, workspace: Option<&str>) -> Result<Id, WgError> {
        match workspace {
            Some(reference) => self.workspace_id(reference),
            None => self.default_workspace_id(),
        }
    }

    /// The workspace a task reference is looked up in: the named one, else every workspace.
    pub fn workspace_or_all(&mut self, workspace: Option<&str>) -> Result<Option<Id>, WgError> {
        workspace
            .map(|reference| self.workspace_id(reference))
            .transpose()
    }

    /// A read-permission check against a fresh stamp.
    pub fn require_read(&mut self, context: &RequestContext) -> Result<(), WgError> {
        self.runtime
            .stamps
            .stamp(context)?
            .require(&[Permission::TaskRead])
    }

    /// A permission check against a fresh stamp, before anything is looked up.
    pub fn require_permission(
        &mut self,
        context: &RequestContext,
        permission: Permission,
    ) -> Result<(), WgError> {
        self.runtime.stamps.stamp(context)?.require(&[permission])
    }
}
