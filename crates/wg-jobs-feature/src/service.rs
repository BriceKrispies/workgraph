//! Shell: the job and worker operations of the HTTP routes (`api/routes/ops.py`). Each one checks
//! its permission first, then resolves what it names, then reads the queue or applies one decision
//! in one transaction. The shapes are the wire DTOs of `wg-wire` (`JobOut`, `WorkerOut`).

use wg_jobs::queue::JobFilter;
use wg_json::Value;
use wg_runtime::App;
use wg_runtimespec::RequestContext;
use wg_runtimespec::jobs as view;
use wg_types::{Id, Permission, WgError, parse_id};
use wg_wire::convention::{counts, dict, int, text};
use wg_wire::dto::{job, worker};

use crate::spec::page_window;

/// The query of `GET /jobs`: the optional workspace, status, kind and task filters and the page.
pub struct JobQuery<'a> {
    pub workspace: Option<&'a str>,
    pub statuses: &'a [String],
    pub kind: Option<&'a str>,
    pub task: Option<&'a str>,
    pub limit: i64,
    pub offset: i64,
}

/// `JobQueue.list` with the stats of the workspace: `{items, total, stats}`. A status that is not
/// a job status, or a task reference that is not an id, matches no job.
pub fn list(
    app: &mut App,
    context: &RequestContext,
    query: &JobQuery<'_>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::WorkspaceRead)?;
    let workspace_id = match query.workspace.filter(|name| !name.is_empty()) {
        Some(reference) => Some(app.workspace_id(reference)?),
        None => None,
    };
    let statuses = view::status_filter(query.statuses);
    // `None` matches nothing; `Some(None)` leaves the task unfiltered.
    let task_id: Option<Option<Id>> = match query.task.filter(|name| !name.is_empty()) {
        Some(reference) => parse_id(reference).ok().map(Some),
        None => Some(None),
    };
    let kind = query.kind.filter(|name| !name.is_empty());
    let (limit, offset) = page_window(query.limit, query.offset);
    let (jobs, total) = match (statuses, task_id) {
        (Some(statuses), Some(task_id)) => app.runtime.store.read(|session| {
            session.list_jobs(&JobFilter {
                statuses: &statuses,
                kind,
                workspace_id,
                task_id,
                offset,
                limit,
            })
        })?,
        _ => (Vec::new(), 0),
    };
    let stats = app
        .runtime
        .store
        .read(|session| session.job_stats(workspace_id))?;
    let pairs: Vec<(String, i64)> = stats
        .into_iter()
        .map(|(status, count)| (status, i64::try_from(count).unwrap_or(i64::MAX)))
        .collect();
    Ok(dict(vec![
        (
            "items",
            Value::List(jobs.iter().map(job::from_model).collect()),
        ),
        ("total", int(i64::try_from(total).unwrap_or(i64::MAX))),
        ("stats", counts(&pairs)),
    ]))
}

/// `JobQueue.get` with its runs: `GET /jobs/{job_id}`.
pub fn get(app: &mut App, context: &RequestContext, job_id: &str) -> Result<Value, WgError> {
    app.require_permission(context, Permission::WorkspaceRead)?;
    let id = view::job_ref(job_id)?;
    let (found, runs) = app
        .runtime
        .store
        .read(|session| Ok((session.job(id)?, session.job_runs(id)?)))?;
    let found = found.ok_or_else(|| view::job_not_found(job_id))?;
    Ok(job::detail_from_model(&found, &runs))
}

/// `JobQueue.cancel`: a queued job is cancelled now, a running one is asked to stop. The reply is
/// `{status}` with the status the job has now.
pub fn cancel(app: &mut App, context: &RequestContext, job_id: &str) -> Result<Value, WgError> {
    app.require_permission(context, Permission::JobManage)?;
    let id = view::job_ref(job_id)?;
    let status = app
        .runtime
        .decide(
            context,
            |session| session.job_slice(id),
            |slice, stamp| wg_jobs::queue::cancel(slice, id, stamp),
        )?
        .reply;
    Ok(dict(vec![("status", text(status.as_str()))]))
}

/// `JobQueue.retry_dead`: re-queues a dead-lettered, failed or cancelled job. The reply is
/// `{status: "queued"}`.
pub fn retry(app: &mut App, context: &RequestContext, job_id: &str) -> Result<Value, WgError> {
    app.require_permission(context, Permission::JobManage)?;
    let id = view::job_ref(job_id)?;
    app.runtime.decide(
        context,
        |session| session.job_slice(id),
        |slice, stamp| wg_jobs::queue::retry_dead(slice, id, stamp),
    )?;
    Ok(dict(vec![("status", text("queued"))]))
}

/// `JobQueue.workers` for `GET /workers`: the workers, newest first.
pub fn workers(app: &mut App, context: &RequestContext) -> Result<Value, WgError> {
    app.require_permission(context, Permission::WorkspaceRead)?;
    let workers = app.runtime.store.read(|session| session.workers())?;
    Ok(Value::List(
        workers.iter().map(worker::from_model).collect(),
    ))
}
