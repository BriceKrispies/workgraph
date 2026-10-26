//! `TaskService` triage apply (`task triage-apply`, CORE-05). The shell reads the triage file,
//! plans the whole array against the task world in one read (`wg_tasks::triage::plan_triage`),
//! then runs each planned verdict's steps in order through the task services and the decisions
//! service. Each service call commits on its own, so a verdict that fails part way keeps the steps
//! that already succeeded and is reported as an error, as in `core/services/triage.py`.

use serde_json::{Value as Json, json};
use wg_json::Value;
use wg_registry::resolve;
use wg_runtime::{App, RequestContext};
use wg_tasks::triage::{PlannedVerdict, Step, TriagePlan, plan_triage, verdict_list};
use wg_types::{ErrorCode, Id, WgError};
use wg_wire::convention::json_value;

use super::{task_add_relation, task_comment, task_create, task_update};
use crate::spec::triage::{
    applied_item, created_key, failure_item, follow_up_body, report, skipped_item,
    step_failure_item, triage_target,
};

/// The entry point of `task triage-apply`: the report of applying (or, for a dry run, planning)
/// the triage file at `path`. The caller exits 1 when the report has errors.
pub fn task_triage_apply(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    path: &str,
    dry_run: bool,
    source: Option<&str>,
) -> Result<Value, WgError> {
    let entries = load_entries(path)?;
    let entries = verdict_list(&entries)?;
    app.require_read(context)?;
    let scope = app.workspace_or_all(workspace)?;
    let (plan, workspaces) = app.runtime.query(
        |session| session.task_world(scope, &[]),
        |world| {
            let plan = plan_triage(entries, source, |key| {
                resolve::task(&world.tasks, scope, key).map(|task| triage_target(world, task))
            });
            let workspaces = plan
                .planned
                .iter()
                .map(|planned| {
                    resolve::task(&world.tasks, scope, &planned.key).map(|task| task.workspace_id)
                })
                .collect::<Result<Vec<Id>, WgError>>()?;
            Ok((plan, workspaces))
        },
    )?;

    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    let mut errors = Vec::new();
    collect_plan(&plan, &mut skipped, &mut errors);
    for (planned, workspace_id) in plan.planned.iter().zip(workspaces) {
        if dry_run {
            applied.push(applied_item(planned, &[]));
            continue;
        }
        let ws = workspace_id.to_string();
        match apply_planned(app, context, &ws, planned) {
            Ok(new_keys) => applied.push(applied_item(planned, &new_keys)),
            Err(error) => errors.push(step_failure_item(planned, &error.message)),
        }
    }
    let shaped = report(applied, skipped, errors);
    Ok(json_value(&shaped))
}

/// Reads the triage file. A file that cannot be read or parsed is a validation failure, as the
/// Python loader raises it.
fn load_entries(path: &str) -> Result<Json, WgError> {
    let unreadable = |err: String| {
        WgError::new(
            ErrorCode::ValidationFailed,
            format!("cannot read triage file {path}: {err}"),
        )
    };
    let text = std::fs::read_to_string(path).map_err(|err| unreadable(err.to_string()))?;
    serde_json::from_str(&text).map_err(|err| unreadable(err.to_string()))
}

/// The report rows planning produced before anything was written: skipped verdicts and the
/// rejected elements, each in file order.
fn collect_plan(plan: &TriagePlan, skipped: &mut Vec<Json>, errors: &mut Vec<Json>) {
    skipped.extend(plan.skipped.iter().map(skipped_item));
    errors.extend(plan.errors.iter().map(failure_item));
}

/// Runs the steps of one planned verdict in order. Returns the keys of the follow-up tasks it
/// created. A failing step stops the verdict; the steps before it stay applied.
fn apply_planned(
    app: &mut App,
    context: &RequestContext,
    ws: &str,
    planned: &PlannedVerdict,
) -> Result<Vec<String>, WgError> {
    let key = planned.key.as_str();
    let mut created: Vec<String> = Vec::new();
    for step in &planned.steps {
        match step {
            Step::RemoveTags(tags) => {
                task_update(app, context, Some(ws), key, &json!({"remove_tags": tags}))?;
            }
            Step::CreateFollowUp(follow) => {
                let out = task_create(app, context, Some(ws), &follow_up_body(follow))?;
                let new_key = created_key(&out);
                task_add_relation(
                    app,
                    context,
                    Some(ws),
                    &new_key,
                    &json!({"target": follow.follows_up, "relation_type": "follows_up"}),
                )?;
                created.push(new_key);
            }
            Step::MarkDuplicate { target } => {
                task_add_relation(
                    app,
                    context,
                    Some(ws),
                    key,
                    &json!({"target": target, "relation_type": "duplicates"}),
                )?;
            }
            Step::Comment(body) => {
                task_comment(app, context, Some(ws), key, &json!({"body": body}))?;
            }
            Step::CommentSuperseded(superseded) => {
                let body = superseded.comment(&created);
                task_comment(app, context, Some(ws), key, &json!({"body": body}))?;
            }
            Step::AddTags(tags) => {
                task_update(app, context, Some(ws), key, &json!({"add_tags": tags}))?;
            }
            Step::RequestDecision { question, options } => {
                wg_decisions::service::request(app, context, Some(ws), key, question, options)?;
            }
            Step::Cancel(reason) => {
                task_update(
                    app,
                    context,
                    Some(ws),
                    key,
                    &json!({"status": "cancelled", "reason": reason}),
                )?;
            }
            Step::CancelSuperseded(superseded) => {
                let reason = superseded.reason(&created);
                task_update(
                    app,
                    context,
                    Some(ws),
                    key,
                    &json!({"status": "cancelled", "reason": reason}),
                )?;
            }
        }
    }
    Ok(created)
}
