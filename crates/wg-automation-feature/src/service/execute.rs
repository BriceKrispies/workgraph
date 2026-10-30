//! `AutomationService.execute_run`: the `automation_action` job's run. The run is claimed in one
//! transaction, its actions run in order (each through the task services, as the automation actor),
//! and the outcome is recorded in a second transaction. A failed action stops the run; the error
//! is raised so the queue retries the job, and a retry runs only the actions not yet finished.

use serde_json::{Map, Value as Json, json};
use wg_exec_feature::service::agents::AgentHost;
use wg_exec_feature::service::run_flow;
use wg_exec_feature::spec::runs::StartRequest;
use wg_intel_feature::service::ops as classification;
use wg_intel_feature::spec::requests::RunOptions;
use wg_jobs::limits::task_verify_keys;
use wg_jobs::queue::NewJob;
use wg_model::RowKey;
use wg_model::entity::{AutomationRule, AutomationRun};
use wg_runtime::{App, RequestContext, system_context};
use wg_store::Session;
use wg_tasks_feature::service as tasks;
use wg_types::{Id, WgError};

use super::{load_rows, reads};
use crate::spec::execute::{self as exec, Claim, Loaded, Running};
use wg_types::pyval::text;

/// The job kind of a verification the `run_verification` action queues (`verify`).
const VERIFY_JOB: &str = "verify";
/// The job kind of the report the `generate_report` action queues.
const REPORT_JOB: &str = "generate_report";

/// Runs the run `run_id`'s actions and records the outcome. The reply is
/// `{"status": ..., "actions": n}`, or the run's status when it was already settled. `host` is the
/// agent host an `enqueue_agent_run` action probes its adapter through (the live one in the worker).
pub fn execute_run(app: &mut App, host: &mut dyn AgentHost, run_id: Id) -> Result<Json, WgError> {
    let system = system_context("automation")?;
    let claimed = app
        .runtime
        .decide(
            &system,
            |session| load_claim(session, run_id),
            |loaded, stamp| exec::claim(stamp, loaded),
        )?
        .reply;
    let running = match claimed {
        Claim::Finished(status) => return Ok(json!({"status": status})),
        Claim::Running(running) => running,
    };
    let actions = exec::action_context(&running)?;
    let slug = app.workspace_slug(&running.workspace_id.to_string())?;
    let mut results = running.kept.clone();
    let mut error = None;
    for (index, action) in running.actions.iter().enumerate() {
        if running.done.contains(&index) {
            continue;
        }
        let kind = text(action.get("type")).to_owned();
        match run_action(app, host, &actions, &slug, &running, action) {
            Ok(output) => results.push(json!({
                "index": index,
                "type": kind,
                "status": "succeeded",
                "output": output,
            })),
            Err(failure) => {
                results.push(json!({
                    "index": index,
                    "type": kind,
                    "status": "failed",
                    "error": failure.message,
                }));
                error = Some(format!(
                    "action {index} ({kind}) failed: {}",
                    failure.message
                ));
                break;
            }
        }
    }
    let rule_name = running.rule_name.clone();
    let run_key = running.run_id;
    let failed = error.clone();
    app.runtime.decide(
        &actions,
        |session| load_run(session, run_key),
        |run, stamp| {
            Ok(exec::finish(
                stamp,
                run,
                &rule_name,
                &results,
                failed.clone(),
            ))
        },
    )?;
    match error {
        Some(message) => Err(WgError::workgraph(message)),
        None => Ok(json!({"status": "succeeded", "actions": results.len()})),
    }
}

/// The rows of a run the claim reads: the run, its rule and the trigger's rows.
fn load_claim(session: &mut Session<'_>, run_id: Id) -> Result<Loaded, WgError> {
    let run = load_run(session, run_id)?;
    let rule = session
        .fetch::<AutomationRule>(&RowKey::AutomationRule(run.rule_id))?
        .ok_or_else(|| WgError::not_found(format!("automation rule {} not found", run.rule_id)))?;
    let event = match run.trigger_event_id {
        Some(event_id) => reads::event(session, event_id)?,
        None => None,
    };
    let rows = load_rows(session, event, &run.inputs)?;
    Ok(Loaded { run, rule, rows })
}

fn load_run(session: &mut Session<'_>, run_id: Id) -> Result<AutomationRun, WgError> {
    session
        .fetch::<AutomationRun>(&RowKey::AutomationRun(run_id))?
        .ok_or_else(|| WgError::not_found(format!("automation run {run_id} not found")))
}

/// Runs one action (`_execute_action`) and returns its output.
fn run_action(
    app: &mut App,
    host: &mut dyn AgentHost,
    context: &RequestContext,
    slug: &str,
    running: &Running,
    action: &serde_json::Map<String, Json>,
) -> Result<Json, WgError> {
    let render_context = &running.context;
    let kind = text(action.get("type"));
    match kind {
        "classify_task" => {
            // `ClassificationService.request(ctx, slug, task)` with the default provider, model
            // and promote flag: queues one `classify_task` job and returns its reply.
            let task = exec::target_task(render_context)?;
            let (job_id, task, status) =
                classification::request(app, context, Some(slug), &task, &RunOptions::default())?;
            Ok(json!({
                "job_id": job_id.to_string(),
                "task_id": task.id.to_string(),
                "task_key": task.key.to_string(),
                "status": status,
            }))
        }
        "add_tags" => {
            let task = exec::target_task(render_context)?;
            let body = exec::tags_body(action, render_context);
            let out = exec::to_json(&tasks::task_update(
                app,
                context,
                Some(slug),
                &task,
                &Json::Object(body),
            )?);
            Ok(json!({"task": out["key"], "tags": out["tags"]}))
        }
        "update_task" => {
            let task = exec::target_task(render_context)?;
            let body = exec::update_body(action);
            let out = exec::to_json(&tasks::task_update(
                app,
                context,
                Some(slug),
                &task,
                &Json::Object(body),
            )?);
            Ok(json!({"task": out["key"], "status": out["status"]}))
        }
        "create_child_tasks" => {
            let parent = exec::target_task(render_context)?;
            let repositories = if exec::inherits_repositories(action) {
                repositories_of(app, &parent)?
            } else {
                Vec::new()
            };
            let sequential = exec::is_sequential(action);
            let mut created: Vec<String> = Vec::new();
            for child in exec::children(action) {
                let body = exec::child_body(
                    &child,
                    &parent,
                    &repositories,
                    if sequential {
                        created.last().map(String::as_str)
                    } else {
                        None
                    },
                    render_context,
                );
                let out = exec::to_json(&tasks::task_create(
                    app,
                    context,
                    Some(slug),
                    &Json::Object(body),
                )?);
                if let Some(key) = out["key"].as_str() {
                    created.push(key.to_owned());
                }
            }
            Ok(json!({"created": created}))
        }
        "create_dependency" => {
            let task = exec::target_task(render_context)?;
            let (reference, depends_on) = exec::dependency_request(action, &task, render_context);
            let out = exec::to_json(&tasks::task_add_dependency(
                app,
                context,
                Some(slug),
                &reference,
                &json!({"depends_on": depends_on}),
            )?);
            Ok(json!({"task": out["key"], "depends_on": out["depends_on"]}))
        }
        "notify" => {
            let workspace = running.workspace_id;
            let note = app
                .runtime
                .decide(
                    context,
                    |_| Ok(()),
                    |_, stamp| Ok(exec::notification(stamp, workspace, action, render_context)),
                )?
                .reply;
            Ok(json!({"notification_id": note.to_string()}))
        }
        "enqueue_agent_run" => {
            let task = exec::target_task(render_context)?;
            let request = StartRequest {
                agent: action_text(action, "agent"),
                model: action_text(action, "model"),
                ..StartRequest::default()
            };
            let attempt = run_flow::run_start(app, context, host, Some(slug), &task, &request)?;
            let out = exec::to_json(&attempt);
            Ok(json!({"attempt_id": out["id"]}))
        }
        "run_verification" => {
            // `task_verify_keys(session, task)`: one verify key per repository the task is linked to.
            let task = exec::target_task(render_context)?;
            let repositories = repositories_of(app, &task)?;
            let job = NewJob {
                payload: Map::from_iter([("task_id".to_owned(), json!(task))]),
                workspace_id: Some(running.workspace_id),
                task_id: Some(wg_types::parse_id(&task)?),
                max_attempts: 1,
                resource_keys: task_verify_keys(repositories.iter().map(String::as_str)),
                created_by: context.actor.to_string(),
                ..NewJob::new(VERIFY_JOB)
            };
            let job_id = app.enqueue_as(context, &job)?;
            Ok(json!({"job_id": job_id.to_string()}))
        }
        "generate_report" => {
            let period_days = action
                .get("period_days")
                .and_then(Json::as_i64)
                .unwrap_or(7);
            let job = NewJob {
                payload: Map::from_iter([
                    ("workspace".to_owned(), json!(slug)),
                    ("scope".to_owned(), json!({"period_days": period_days})),
                ]),
                workspace_id: Some(running.workspace_id),
                max_attempts: 2,
                created_by: context.actor.to_string(),
                ..NewJob::new(REPORT_JOB)
            };
            let job_id = app.enqueue_as(context, &job)?;
            Ok(json!({"job_id": job_id.to_string()}))
        }
        other => Err(WgError::workgraph(format!(
            "automation action {other} is not ported to Rust yet"
        ))),
    }
}

/// The text of an optional action field: `None` when absent or not a string.
fn action_text(action: &Map<String, Json>, name: &str) -> Option<String> {
    action.get(name).and_then(Json::as_str).map(str::to_owned)
}

/// The repositories a task is linked to, as ids (`TaskRepository.repository_id`).
fn repositories_of(app: &mut App, task: &str) -> Result<Vec<String>, WgError> {
    let task_id = wg_types::parse_id(task)?;
    app.runtime.store.read(|session| {
        let world = session.task_world(None, &[])?;
        Ok(world
            .tasks
            .task_repositories
            .keys()
            .filter(|(linked, _)| *linked == task_id)
            .map(|(_, repository)| repository.to_string())
            .collect::<Vec<String>>())
    })
}
