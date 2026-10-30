//! Task-family reads and `task_fts` maintenance on a [`Session`] (SH-04).
//!
//! The statements come from `wg_rows::task_sql`; this module only runs them and decodes the
//! rows. A session is one transaction, so [`Session::index_task`] runs inside whatever write the
//! caller is making and sees that write's changes.

use wg_model::RowKey;
use wg_model::entity::{
    Classification, Task, TaskComment, TaskDependency, TaskFile, TaskLink, TaskRelation, TaskTag,
};
use wg_rows::{Statement, TableRow, task_sql};
use wg_types::{Id, TaskKey, TaskStatus, WgError};

use crate::Session;

impl Session<'_> {
    /// Runs a query and decodes every row through the table's codec.
    fn load_all<T: TableRow>(&mut self, statement: &Statement) -> Result<Vec<T>, WgError> {
        self.rows(statement)?
            .iter()
            .map(|values| T::from_raw(values))
            .collect()
    }

    /// The task with `key` in `workspace_id`, if there is one.
    pub fn task_by_key(
        &mut self,
        workspace_id: Id,
        key: &TaskKey,
    ) -> Result<Option<Task>, WgError> {
        Ok(self
            .load_all::<Task>(&task_sql::task_by_key(workspace_id, key))?
            .pop())
    }

    /// Every task with `key` in any workspace, in workspace and number order.
    pub fn tasks_by_key(&mut self, key: &TaskKey) -> Result<Vec<Task>, WgError> {
        self.load_all(&task_sql::tasks_by_key(key))
    }

    /// The tasks of a workspace in number order, optionally only those in `status`.
    pub fn tasks_in_workspace(
        &mut self,
        workspace_id: Id,
        status: Option<TaskStatus>,
    ) -> Result<Vec<Task>, WgError> {
        self.load_all(&task_sql::tasks_in_workspace(workspace_id, status))
    }

    /// The files attached to a task, in position order.
    pub fn task_files(&mut self, task_id: Id) -> Result<Vec<TaskFile>, WgError> {
        self.load_all(&task_sql::task_files(task_id))
    }

    /// The tags of a task, sorted by tag.
    pub fn task_tags(&mut self, task_id: Id) -> Result<Vec<TaskTag>, WgError> {
        self.load_all(&task_sql::task_tags(task_id))
    }

    /// The external links of a task, oldest first.
    pub fn task_links(&mut self, task_id: Id) -> Result<Vec<TaskLink>, WgError> {
        self.load_all(&task_sql::task_links(task_id))
    }

    /// The comments of a task, oldest first.
    pub fn task_comments(&mut self, task_id: Id) -> Result<Vec<TaskComment>, WgError> {
        self.load_all(&task_sql::task_comments(task_id))
    }

    /// The dependencies a task waits on.
    pub fn task_dependencies(&mut self, task_id: Id) -> Result<Vec<TaskDependency>, WgError> {
        self.load_all(&task_sql::task_dependencies(task_id))
    }

    /// The dependencies that wait on a task.
    pub fn task_dependents(&mut self, task_id: Id) -> Result<Vec<TaskDependency>, WgError> {
        self.load_all(&task_sql::task_dependents(task_id))
    }

    /// The relations a task takes part in, from either end.
    pub fn task_relations(&mut self, task_id: Id) -> Result<Vec<TaskRelation>, WgError> {
        self.load_all(&task_sql::task_relations(task_id))
    }

    /// Rebuilds the `task_fts` row for `task_id` from the task's current rows. A task that no
    /// longer exists keeps no index row.
    pub fn index_task(&mut self, task_id: Id) -> Result<(), WgError> {
        self.run(&task_sql::fts_delete(task_id))?;
        let Some(task) = self.fetch::<Task>(&RowKey::Task(task_id))? else {
            return Ok(());
        };
        let comments = self.task_comments(task_id)?;
        let tags = self.task_tags(task_id)?;
        let classification = match task.current_classification_id {
            Some(id) => self
                .load_all::<Classification>(&task_sql::classification_by_id(id))?
                .pop(),
            None => None,
        };
        let doc = task_sql::task_index_doc(&task, &comments, &tags, classification.as_ref());
        self.run(&task_sql::fts_insert(&doc))?;
        Ok(())
    }
}
