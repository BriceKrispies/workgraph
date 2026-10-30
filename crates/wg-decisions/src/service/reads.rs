//! Structured-decision reads on a [`Session`] (`task_decisions`).
//!
//! The statements come from [`crate::spec::sql`]; these functions only run them and decode the
//! rows. Writes go through `Session::apply_decision` like every other row.

use wg_model::entity::TaskDecision;
use wg_rows::Raw;
use wg_rows::rows::TableRow;
use wg_store::Session;
use wg_types::{Id, WgError};

use crate::spec::sql::{
    self, DecisionFilter, DecisionWithTask, attach_tags, decode_decision_with_task, decode_task_tag,
};

/// Runs a query and decodes every row as a decision.
fn decisions(
    session: &mut Session<'_>,
    statement: &wg_rows::Statement,
) -> Result<Vec<TaskDecision>, WgError> {
    session
        .rows(statement)?
        .iter()
        .map(|values| TaskDecision::from_raw(values))
        .collect()
}

/// Runs a brief query (decision columns, then the task brief) and decodes every row. Tags are
/// read by a second query over the tasks found, as Python loads `Task.tags`.
fn decisions_with_task(
    session: &mut Session<'_>,
    statement: &wg_rows::Statement,
) -> Result<Vec<DecisionWithTask>, WgError> {
    let rows = session
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
    let tags = session
        .rows(&sql::task_tags_of(&task_ids))?
        .iter()
        .map(|values| decode_task_tag(values))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(attach_tags(&rows, &tags))
}

/// The decisions of a task, oldest first.
pub fn decisions_for_task(
    session: &mut Session<'_>,
    task_id: Id,
) -> Result<Vec<TaskDecision>, WgError> {
    decisions(session, &sql::decisions_for_task(task_id))
}

/// The decisions of a workspace with the status `filter` selects, at most `limit` of them, in
/// the order `filter` names (see [`crate::spec::sql`]).
pub fn decisions_by_status(
    session: &mut Session<'_>,
    workspace_id: Id,
    filter: DecisionFilter,
    limit: i64,
) -> Result<Vec<TaskDecision>, WgError> {
    decisions(
        session,
        &sql::decisions_by_status(workspace_id, filter, limit),
    )
}

/// [`decisions_by_status`] with each decision's task brief (`DecisionWithTaskOut`).
pub fn decisions_by_status_with_task(
    session: &mut Session<'_>,
    workspace_id: Id,
    filter: DecisionFilter,
    limit: i64,
) -> Result<Vec<DecisionWithTask>, WgError> {
    decisions_with_task(
        session,
        &sql::decisions_by_status_with_task(workspace_id, filter, limit),
    )
}

/// The decision with `decision_id` in `workspace_id`, if there is one.
pub fn decision_by_id(
    session: &mut Session<'_>,
    workspace_id: Id,
    decision_id: Id,
) -> Result<Option<TaskDecision>, WgError> {
    Ok(decisions(session, &sql::decision_by_id(workspace_id, decision_id))?.pop())
}

/// [`decision_by_id`] with the task brief.
pub fn decision_by_id_with_task(
    session: &mut Session<'_>,
    workspace_id: Id,
    decision_id: Id,
) -> Result<Option<DecisionWithTask>, WgError> {
    Ok(decisions_with_task(
        session,
        &sql::decision_by_id_with_task(workspace_id, decision_id),
    )?
    .pop())
}

/// How many of a task's decisions are open.
pub fn open_decision_count(session: &mut Session<'_>, task_id: Id) -> Result<i64, WgError> {
    match session
        .rows(&sql::open_decision_count(task_id))?
        .first()
        .and_then(|row| row.first())
    {
        Some(Raw::Int(count)) => Ok(*count),
        _ => Err(WgError::workgraph(
            "open decision count returned no integer",
        )),
    }
}
