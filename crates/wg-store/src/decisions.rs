//! Structured-decision reads on a [`Session`] (`task_decisions`).
//!
//! The statements come from `wg_storecodec::decision_sql`; this module only runs them and decodes
//! the rows. Writes go through [`Session::apply_decision`] like every other row.

use wg_model::entity::TaskDecision;
use wg_storecodec::decision_sql::{self, DecisionFilter};
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
