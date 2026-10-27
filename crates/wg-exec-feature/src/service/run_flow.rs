//! Shell: the entry points that queue and cancel managed runs (`RunService.start`, `cancel`,
//! `retry` and `rerun` of `orchestration/runs.py`). Each one checks the permission and the
//! execution opt-in, resolves what it names, probes the agent adapter before any state changes,
//! and applies the pure decision of [`crate::spec::runs`] in one transaction.

use std::path::PathBuf;

use serde_json::{Map, Value as Json};
use wg_exec::run::{require_execution, rerun_plan, retry_check};
use wg_git::Git;
use wg_json::Value;
use wg_registry::resolve;
use wg_runtime::{App, index_written};
use wg_runtimespec::RequestContext;
use wg_runtimespec::effects::GIT_TIMEOUT_MS;
use wg_types::{Permission, WgError};

use crate::service::agents::{AgentHost, require_available};
use crate::service::runs::{agent_job, find_attempt};
use crate::spec::config::{config_at, execution_enabled, execution_text};
use crate::spec::runs::{
    StartInputs, StartRequest, baseline_facts, baseline_sources, cancel_run, count_retry,
    rerun_document, rerun_request, retry_request, start_run,
};

/// `RunService._require_execution`: the permission, then the opt-in.
fn require_run(app: &mut App, context: &RequestContext) -> Result<Map<String, Json>, WgError> {
    app.require_permission(context, Permission::RunStart)?;
    let config = app.config()?;
    require_execution(execution_enabled(&config))?;
    Ok(config)
}

/// `RunService.start`: claims the task for the agent when it is ready or failed, records a queued
/// attempt and queues its `agent_run` job. The adapter is probed first, so a missing agent leaves
/// no state behind. The reply is the attempt as `AttemptOut`.
pub fn run_start(
    app: &mut App,
    context: &RequestContext,
    host: &mut dyn AgentHost,
    workspace: Option<&str>,
    task_ref: &str,
    request: &StartRequest,
) -> Result<Value, WgError> {
    let config = require_run(app, context)?;
    let scope = app.workspace_or_all(workspace)?;
    let (task_id, workspace_id, assigned) = app.runtime.store.read(|session| {
        let world = session.task_world(scope, &[])?;
        let task = resolve::task(&world.tasks, scope, task_ref)?;
        Ok((task.id, task.workspace_id, task.assigned_agent.clone()))
    })?;
    let slug = app.workspace_slug(&workspace_id.to_string())?;
    let effective = app.effective_config(Some(&slug))?;
    let agent = request
        .agent
        .clone()
        .filter(|name| !name.is_empty())
        .or(assigned.filter(|name| !name.is_empty()))
        .or_else(|| {
            config_at(&effective, &["execution", "default_agent"])
                .and_then(Json::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "claude_code".to_owned());
    let provider = require_available(host, &effective, &agent)?;
    let inputs = StartInputs {
        agent,
        adapter: provider.adapter.to_owned(),
        provider_snapshot: provider.snapshot,
        model: request.model.clone(),
        instructions: request.instructions.clone(),
        repositories: request.repositories.clone(),
        retry_of: request.retry_of,
        rerun_of: request.rerun_of,
        parent_attempt: request.parent_attempt,
        priority: request.priority,
        isolation: execution_text(&config, "isolation", "worktree"),
    };
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| {
                let world = session.task_world(Some(workspace_id), &[])?;
                let attempts = session.attempts_of_task(task_id, &[])?;
                Ok((world, attempts))
            },
            |(world, attempts), stamp| start_run(world, attempts, task_id, stamp, &inputs),
            index_written,
        )?
        .reply)
}

/// `RunService.cancel`: asks a queued or running attempt to stop (a queued one stops at once).
pub fn run_cancel(
    app: &mut App,
    context: &RequestContext,
    attempt_ref: &str,
    reason: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::RunCancel)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| {
                let attempt = find_attempt(session, attempt_ref)?;
                let job = agent_job(session, &attempt)?;
                let world = session.task_world(Some(attempt.workspace_id), &[])?;
                Ok((attempt, job, world))
            },
            |(attempt, job, world), stamp| cancel_run(world, attempt, job.as_ref(), reason, stamp),
            index_written,
        )?
        .reply)
}

/// `RunService.retry`: a new attempt after a failed one, with the same agent and model.
pub fn run_retry(
    app: &mut App,
    context: &RequestContext,
    host: &mut dyn AgentHost,
    attempt_ref: &str,
    instructions: Option<&str>,
) -> Result<Value, WgError> {
    let previous = app
        .runtime
        .store
        .read(|session| find_attempt(session, attempt_ref))?;
    retry_check(previous.status)?;
    app.runtime.decide(
        context,
        |session| find_attempt(session, attempt_ref),
        |fresh, stamp| Ok(count_retry(fresh, stamp)),
    )?;
    run_start(
        app,
        context,
        host,
        None,
        &previous.task_id.to_string(),
        &retry_request(&previous, instructions),
    )
}

/// `RunService.rerun`: a new attempt that reuses the recorded configuration and the baseline
/// revisions that are still available. Model execution is nondeterministic: a rerun reproduces the
/// inputs, not the output.
pub fn run_rerun(
    app: &mut App,
    context: &RequestContext,
    host: &mut dyn AgentHost,
    attempt_ref: &str,
) -> Result<Value, WgError> {
    let (previous, baseline) = app.runtime.store.read(|session| {
        let previous = find_attempt(session, attempt_ref)?;
        let baseline = match previous.baseline_manifest_id {
            Some(id) => session.manifest_by_id(id)?,
            None => None,
        };
        Ok((previous, baseline))
    })?;
    let entries: Vec<Map<String, Json>> = baseline
        .map(|manifest| manifest.entries.iter().cloned().collect())
        .unwrap_or_default();
    // Whether each source checkout is a repository that still holds its revision.
    let available: Vec<bool> = baseline_sources(&entries)
        .into_iter()
        .map(|(source, sha)| match (source, sha) {
            (Some(source), Some(sha)) => {
                let git = Git {
                    path: PathBuf::from(source),
                    timeout_ms: GIT_TIMEOUT_MS,
                };
                git.is_repository().unwrap_or(false) && git.object_exists(&sha).unwrap_or(false)
            }
            _ => false,
        })
        .collect();
    let plan = rerun_plan(&baseline_facts(&entries, &available));
    let started = run_start(
        app,
        context,
        host,
        None,
        &previous.task_id.to_string(),
        &rerun_request(&previous, &plan),
    )?;
    Ok(rerun_document(started, &plan))
}
