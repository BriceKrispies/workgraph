//! Structured-decision reads on a [`Session`] (`task_decisions`).
//!
//! The statements come from `wg_storecodec::decision_sql`; this module only runs them and decodes
//! the rows. Writes go through [`Session::apply_decision`] like every other row.

use wg_model::entity::TaskDecision;
use wg_storecodec::decision_sql::{
    self, DecisionFilter, DecisionWithTask, attach_tags, decode_decision_with_task, decode_task_tag,
};
use wg_storecodec::{Raw, Statement, TableRow};
use wg_types::{ErrorCode, Id, WgError};

use crate::Session;

impl Session<'_> {
    /// Runs a query and decodes every row as a decision.
    fn decisions(&mut self, statement: &Statement) -> Result<Vec<TaskDecision>, WgError> {
        self.rows(statement)?
            .iter()
            .map(|values| TaskDecision::from_raw(values))
            .collect()
    }

    /// Runs a brief query (decision columns, then the task brief) and decodes every row. Tags are
    /// read by a second query over the tasks found, as Python loads `Task.tags`.
    fn decisions_with_task(
        &mut self,
        statement: &Statement,
    ) -> Result<Vec<DecisionWithTask>, WgError> {
        let mut rows = self
            .rows(statement)?
            .iter()
            .map(|values| decode_decision_with_task(values))
            .collect::<Result<Vec<_>, _>>()?;
        if rows.is_empty() {
            return Ok(rows);
        }
        let mut task_ids: Vec<Id> = rows.iter().map(|row| row.decision.task_id).collect();
        task_ids.sort();
        task_ids.dedup();
        let tags = self
            .rows(&decision_sql::task_tags_of(&task_ids))?
            .iter()
            .map(|values| decode_task_tag(values))
            .collect::<Result<Vec<_>, _>>()?;
        attach_tags(&mut rows, &tags);
        Ok(rows)
    }

    /// The decisions of a task, oldest first.
    pub fn decisions_for_task(&mut self, task_id: Id) -> Result<Vec<TaskDecision>, WgError> {
        self.decisions(&decision_sql::decisions_for_task(task_id))
    }

    /// The decisions of a workspace with the status `filter` selects, at most `limit` of them, in
    /// the order `filter` names (see `wg_storecodec::decision_sql`).
    pub fn decisions_by_status(
        &mut self,
        workspace_id: Id,
        filter: DecisionFilter,
        limit: i64,
    ) -> Result<Vec<TaskDecision>, WgError> {
        self.decisions(&decision_sql::decisions_by_status(
            workspace_id,
            filter,
            limit,
        ))
    }

    /// [`Session::decisions_by_status`] with each decision's task brief (`DecisionWithTaskOut`).
    pub fn decisions_by_status_with_task(
        &mut self,
        workspace_id: Id,
        filter: DecisionFilter,
        limit: i64,
    ) -> Result<Vec<DecisionWithTask>, WgError> {
        self.decisions_with_task(&decision_sql::decisions_by_status_with_task(
            workspace_id,
            filter,
            limit,
        ))
    }

    /// The decision with `decision_id` in `workspace_id`, if there is one.
    pub fn decision_by_id(
        &mut self,
        workspace_id: Id,
        decision_id: Id,
    ) -> Result<Option<TaskDecision>, WgError> {
        Ok(self
            .decisions(&decision_sql::decision_by_id(workspace_id, decision_id))?
            .pop())
    }

    /// [`Session::decision_by_id`] with the task brief.
    pub fn decision_by_id_with_task(
        &mut self,
        workspace_id: Id,
        decision_id: Id,
    ) -> Result<Option<DecisionWithTask>, WgError> {
        Ok(self
            .decisions_with_task(&decision_sql::decision_by_id_with_task(
                workspace_id,
                decision_id,
            ))?
            .pop())
    }

    /// How many of a task's decisions are open.
    pub fn open_decision_count(&mut self, task_id: Id) -> Result<i64, WgError> {
        match self
            .rows(&decision_sql::open_decision_count(task_id))?
            .first()
            .and_then(|row| row.first())
        {
            Some(Raw::Int(count)) => Ok(*count),
            _ => Err(WgError::new(
                ErrorCode::WorkgraphError,
                "open decision count returned no integer",
            )),
        }
    }
}
