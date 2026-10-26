//! Loads the task world (IF-05): the slices a task command decides on.
//!
//! The statements come from `wg_storecodec::task_world_sql`; this module runs them and decodes
//! each row through its table's codec.

use std::collections::BTreeSet;

use wg_model::entity::{
    ExternalItem, Project, Repository, Stack, StackItem, Task, TaskComment, TaskDependency,
    TaskFile, TaskLink, TaskProject, TaskRelation, TaskRepository, TaskTag, Workspace,
};
use wg_model::{Row, Table};
use wg_storecodec::task_world_sql::{TaskWorld, fts_task_ids, statements};
use wg_storecodec::{Raw, TableRow};
use wg_types::{ErrorCode, Id, WgError, parse_id};

use crate::Session;

/// Decodes one result row of `table` into a [`Row`].
fn decode(table: Table, values: &[wg_storecodec::Raw]) -> Result<Row, WgError> {
    macro_rules! by_table {
        ($($variant:ident),+ $(,)?) => {
            match table {
                $(Table::$variant => Ok(Row::from($variant::from_raw(values)?)),)+
                other => Err(WgError::new(
                    ErrorCode::WorkgraphError,
                    format!("the task world does not load {}", other.name()),
                )),
            }
        };
    }
    by_table!(
        Workspace,
        Project,
        Repository,
        Task,
        TaskFile,
        TaskTag,
        TaskLink,
        TaskComment,
        TaskDependency,
        TaskRelation,
        TaskProject,
        TaskRepository,
        ExternalItem,
        Stack,
        StackItem,
    )
}

impl Session<'_> {
    /// The ids of the tasks whose full-text row matches `match_expr`.
    pub fn fts_task_ids(&mut self, match_expr: &str) -> Result<BTreeSet<Id>, WgError> {
        self.rows(&fts_task_ids(match_expr))?
            .iter()
            .map(|values| match values.first() {
                Some(Raw::Text(text)) => parse_id(text),
                _ => Err(WgError::new(
                    ErrorCode::WorkgraphError,
                    "task_fts returned no task id",
                )),
            })
            .collect()
    }

    /// The task world of `workspace` (`None`: every workspace), with the comments of
    /// `comments_for`.
    pub fn task_world(
        &mut self,
        workspace: Option<Id>,
        comments_for: &[Id],
    ) -> Result<TaskWorld, WgError> {
        let mut world = TaskWorld::default();
        for (table, statement) in statements(workspace, comments_for) {
            let rows = self
                .rows(&statement)?
                .iter()
                .map(|values| decode(table, values))
                .collect::<Result<Vec<_>, _>>()?;
            world = world.with_rows(rows);
        }
        Ok(world)
    }
}
