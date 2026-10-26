//! The decision entry points of the application: the services behind `decision list|show|request|
//! choose|withdraw`, the `/decisions` routes and the decision tools (`DecisionService`).
//!
//! Each one resolves the workspace, loads what the call reads in the transaction of the call,
//! hands it to the pure function of `wg_runtimespec::decisions` and applies what it returns. The
//! writes keep the full-text index of the tasks they touch in step, as the task entry points do.

use serde_json::Value as Json;
use wg_json::Value;
use wg_model::RowKey;
use wg_model::entity::TaskDecision;
use wg_runtimespec::RequestContext;
use wg_runtimespec::decisions::{self as spec, DecisionFilter, DecisionWorld};
use wg_runtimespec::tasks::reads::resolve_task_id;
use wg_store::Session;
use wg_tasks::decisions::Answer;
use wg_types::{ErrorCode, Id, Permission, WgError};

use crate::app::App;
use crate::tasks::index_written;

/// The decision `reference` names in the workspace `scope` (any workspace when `None`), with the
/// task world of its workspace and the decisions on its task.
fn load_decision(
    session: &mut Session<'_>,
    scope: Option<Id>,
    reference: &str,
) -> Result<DecisionWorld, WgError> {
    let found = match spec::decision_key(reference) {
        Some(id) => session.fetch::<TaskDecision>(&RowKey::TaskDecision(id))?,
        None => None,
    };
    let decision = spec::locate(found, scope, reference)?;
    Ok(DecisionWorld {
        world: session.task_world(Some(decision.workspace_id), &[])?,
        siblings: session.decisions_for_task(decision.task_id)?,
        decision,
    })
}

impl App {
    /// `DecisionService.request`: opens a decision on a task and tags the task `needs-decision`.
    pub fn decision_request(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        task: &str,
        question: &str,
        options: &[Json],
    ) -> Result<Value, WgError> {
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
            .runtime
            .decide_then(
                context,
                |session| session.task_world(scope, &[]),
                |world, stamp| spec::request(world, stamp, scope, task, question, options),
                index_written,
            )?
            .reply)
    }

    /// `DecisionService.list`: decisions with their task.
    pub fn decision_list(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        filter: DecisionFilter,
        limit: i64,
    ) -> Result<Value, WgError> {
        self.require_read(context)?;
        let scope = self.workspace_or_default(workspace)?;
        self.runtime.query(
            |session| session.decisions_by_status_with_task(scope, filter, limit),
            |rows| Ok(spec::list_out(rows)),
        )
    }

    /// `DecisionService.get`: one decision with its task.
    pub fn decision_get(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        decision_id: &str,
    ) -> Result<Value, WgError> {
        self.require_read(context)?;
        let scope = self.workspace_or_all(workspace)?;
        self.runtime.query(
            |session| {
                let found = match spec::decision_key(decision_id) {
                    Some(id) => session.fetch::<TaskDecision>(&RowKey::TaskDecision(id))?,
                    None => None,
                };
                let row = spec::locate(found, scope, decision_id)?;
                session.decision_by_id_with_task(row.workspace_id, row.id)
            },
            |row| {
                row.as_ref().map(spec::with_task_out).ok_or_else(|| {
                    WgError::new(ErrorCode::WorkgraphError, "the decision's task vanished")
                })
            },
        )
    }

    /// `DecisionService.for_task`: every decision on a task, oldest first.
    pub fn decision_for_task(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        task: &str,
    ) -> Result<Value, WgError> {
        self.require_read(context)?;
        let scope = self.workspace_or_all(workspace)?;
        self.runtime.query(
            |session| {
                let world = session.task_world(scope, &[])?;
                let id = resolve_task_id(&world, scope, task)?;
                let key = world
                    .tasks
                    .task(id)
                    .map(|found| found.key.to_string())
                    .unwrap_or_default();
                Ok((key, session.decisions_for_task(id)?))
            },
            |(key, rows)| Ok(spec::for_task_out(key, rows)),
        )
    }

    /// `DecisionService.choose`: answers a decision with an option or free text.
    pub fn decision_choose(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        decision_id: &str,
        option_id: Option<&str>,
        other_text: Option<&str>,
        note: Option<&str>,
    ) -> Result<Value, WgError> {
        let answer = Answer {
            option_id,
            other_text,
            note,
        };
        let stamp = self.runtime.stamps.stamp(context)?;
        spec::check_choose(&stamp, option_id, other_text)?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
            .runtime
            .decide_then(
                context,
                |session| load_decision(session, scope, decision_id),
                |load, stamp| spec::choose(load, stamp, answer),
                index_written,
            )?
            .reply)
    }

    /// `DecisionService.withdraw`: retires an open decision.
    pub fn decision_withdraw(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        decision_id: &str,
        reason: Option<&str>,
    ) -> Result<Value, WgError> {
        let stamp = self.runtime.stamps.stamp(context)?;
        stamp.require(&[Permission::TaskWrite])?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
            .runtime
            .decide_then(
                context,
                |session| load_decision(session, scope, decision_id),
                |load, stamp| spec::withdraw(load, stamp, reason),
                index_written,
            )?
            .reply)
    }
}
