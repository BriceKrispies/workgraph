//! Loads the task world (IF-05): the slices a task command decides on.
//!
//! The statements come from `wg_storecodec::task_world_sql`; this module runs them and decodes
//! each row through its table's codec.

use std::collections::BTreeSet;

use wg_storecodec::Raw;
use wg_storecodec::task_world_sql::{TaskWorld, decode_row, fts_task_ids, statements};
use wg_types::{Id, WgError, parse_id};

use crate::Session;

impl Session<'_> {
    /// The ids of the tasks whose full-text row matches `match_expr`.
    pub fn fts_task_ids(&mut self, match_expr: &str) -> Result<BTreeSet<Id>, WgError> {
        self.rows(&fts_task_ids(match_expr))?
            .iter()
            .map(|values| match values.first() {
                Some(Raw::Text(text)) => parse_id(text),
                _ => Err(WgError::workgraph("task_fts returned no task id")),
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
                .map(|values| decode_row(table, values))
                .collect::<Result<Vec<_>, _>>()?;
            world = world.with_rows(rows);
        }
        Ok(world)
    }
}
