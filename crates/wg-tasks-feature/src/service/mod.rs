//! The task entry points of the application (`TaskService`): the services behind `task create`,
//! `list`, `show`, `update`, `claim`, `comment`, the dependencies, relations and repositories of
//! a task. Each one resolves the workspace, runs the decision of the pure layer over the task world
//! and applies what it returns. These are free functions over `&mut App`, reached from the
//! feature's `cli`, `http` and `mcp` tables.

pub mod acceptance;
pub mod rename;

use crate::spec::bodies::{
    read_claim, read_comment, read_complete, read_dependency, read_progress, read_reason,
    read_relation, read_repo, read_result,
};
use crate::spec::claims::{self, Reported};
use crate::spec::links;
use crate::spec::reads::{self, GraphRequest, Shown};
use crate::spec::requests::{
    read_create, read_filter, validate_bulk, validate_create, validate_filter,
};
use crate::spec::writes;
use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_model::slice::QueueSlice;
use wg_model::{Decision, Row, Stamp};
use wg_registry::resolve;
use wg_runtime::{App, index_written};
use wg_runtimespec::RequestContext;
use wg_store::{EventQuery, Session, TaskWorld};
use wg_tasks::bulk::bulk_results;
use wg_tasks::edit::{self, read_update, validate_update};
use wg_tasks::shape::{bulk_result_out, key_of};
use wg_types::{ErrorCode, Id, Permission, WgError, parse_id};

/// `TaskService.create`.
pub fn task_create(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    body: &Json,
) -> Result<Value, WgError> {
    let read = read_create(&validate_create(body)?)?;
    let enrich = edit::enrichment_from(&app.config()?);
    let scope = app.workspace_or_default(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(Some(scope), &[]),
            |world: &TaskWorld, stamp| writes::create_task(world, stamp, scope, read, enrich),
            index_written,
        )?
        .reply)
}

/// `TaskService.update`.
pub fn task_update(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    body: &Json,
) -> Result<Value, WgError> {
    let dump = validate_update(body)?;
    let given = body.as_object().cloned().unwrap_or_default();
    let enrich = edit::enrichment_from(&app.config()?);
    update_checked(app, context, workspace, reference, &dump, &given, enrich)
}

fn update_checked(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    dump: &Map<String, Json>,
    given: &Map<String, Json>,
    enrich: edit::Enrichment,
) -> Result<Value, WgError> {
    let read = read_update(dump, given)?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| {
                let world = session.task_world(scope, &[])?;
                // The jobs already stored for the task, which an enrichment enqueue dedupes
                // against. An unresolved reference is left to the decision to report.
                let queue = match resolve::task(&world.tasks, scope, reference) {
                    Ok(task) => session.task_jobs_slice(task.id)?,
                    Err(_) => QueueSlice::default(),
                };
                Ok((world, queue))
            },
            |(world, queue): &(TaskWorld, QueueSlice), stamp| {
                edit::update_task(world, queue, stamp, scope, reference, read, enrich)
            },
            index_written,
        )?
        .reply)
}

/// `TaskService.bulk_update`: one update applied to many tasks, each in its own transaction.
pub fn task_bulk_update(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    body: &Json,
) -> Result<Value, WgError> {
    let dump = validate_bulk(body)?;
    let update = dump
        .get("update")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let given = body
        .get("update")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let refs: Vec<String> = dump
        .get("tasks")
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let enrich = edit::enrichment_from(&app.config()?);
    let outcomes = refs
        .into_iter()
        .map(|reference| {
            let mut shared = update.clone();
            shared.insert("expected_version".to_owned(), Json::Null);
            let done = update_checked(app, context, workspace, &reference, &shared, &given, enrich)
                .and_then(|out| {
                    key_of(&out).ok_or_else(|| {
                        WgError::new(ErrorCode::WorkgraphError, "a task output has no key")
                    })
                });
            (reference, done)
        })
        .collect::<Vec<_>>();
    Ok(bulk_result_out(&bulk_results(outcomes)?))
}

/// `TaskService.archive`.
pub fn task_archive(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    archived: bool,
) -> Result<Value, WgError> {
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(scope, &[]),
            |world: &TaskWorld, stamp| {
                writes::archive_task(world, stamp, scope, reference, archived)
            },
            index_written,
        )?
        .reply)
}

/// `TaskService.list`: a page of tasks.
pub fn task_list(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    body: &Json,
) -> Result<Value, WgError> {
    let filter = read_filter(&validate_filter(body)?)?;
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    let expression = reads::filter_match(&filter);
    app.runtime.query(
        |session| {
            let world = session.task_world(Some(scope), &[])?;
            let matched = expression
                .as_deref()
                .map(|expr| session.fts_task_ids(expr))
                .transpose()?;
            Ok((world, matched))
        },
        |(world, matched)| reads::list_tasks(world, scope, &filter, matched.as_ref()),
    )
}

/// `TaskService.get_ready`.
pub fn task_ready(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    project: Option<&str>,
    repository: Option<&str>,
    agent: Option<&str>,
    limit: i64,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| session.task_world(Some(scope), &[]),
        |world| reads::ready(world, scope, project, repository, agent, limit),
    )
}

/// `TaskService.detail` for each reference.
pub fn task_show(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    refs: &[String],
) -> Result<Shown, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_all(workspace)?;
    app.runtime.query(
        |session| {
            let world = session.task_world(scope, &[])?;
            let shown = reads::shown_ids(&world, scope, refs);
            let mut comments = Vec::new();
            for task_id in shown {
                comments.extend(session.task_comments(task_id)?.into_iter().map(Row::from));
            }
            Ok(world.with_rows(comments))
        },
        |world| Ok(reads::show(world, scope, refs)),
    )
}

/// `TaskService.graph`.
pub fn task_graph(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    request: &GraphRequest<'_>,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| session.task_world(Some(scope), &[]),
        |world| reads::graph(world, scope, request),
    )
}

/// `HistoryService.task_history`: the events of a task and of its attempts, oldest first.
pub fn task_history(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    event_types: &[String],
) -> Result<Value, WgError> {
    task_history_limited(app, context, workspace, reference, event_types, 500)
}

/// [`task_history`](Self::task_history) cut to the first `limit` events.
pub fn task_history_limited(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    event_types: &[String],
    limit: i64,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_all(workspace)?;
    app.runtime.query(
        |session| {
            let world = session.task_world(scope, &[])?;
            let task = resolve::task(&world.tasks, scope, reference).map(|task| task.id)?;
            let mut ids = vec![task];
            ids.extend(
                session
                    .attempts_of_task(task, &[])?
                    .into_iter()
                    .map(|attempt| attempt.id),
            );
            session.query_events(&EventQuery {
                entity_ids: ids,
                event_types: event_types.to_vec(),
                limit,
                ..EventQuery::default()
            })
        },
        |events| {
            Ok(Value::List(
                events
                    .iter()
                    .map(wg_runtimespec::events::event_out)
                    .collect(),
            ))
        },
    )
}

/// `TaskService.promote_ready_report`, with the `dry_run` flag added.
pub fn task_promote_ready(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    tags: &[String],
    project: Option<&str>,
    dry_run: bool,
) -> Result<Value, WgError> {
    let scope = app.workspace_or_default(workspace)?;
    if dry_run {
        let stamp = app.runtime.stamps.stamp(context)?;
        return app.runtime.query(
            |session| session.task_world(Some(scope), &[]),
            |world| {
                writes::promote_ready(world, &stamp, scope, tags, project, true)
                    .map(|decision| decision.reply)
            },
        );
    }
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(Some(scope), &[]),
            |world: &TaskWorld, stamp| {
                writes::promote_ready(world, stamp, scope, tags, project, false)
            },
            index_written,
        )?
        .reply)
}

/// Runs a decision over the task world that also reads the verification configurations of the
/// task's workspace and the attempt `attempt_id` names (complete, result, comment, progress).
fn task_reported(
    app: &mut App,
    context: &RequestContext,
    scope: Option<Id>,
    reference: &str,
    attempt_id: Option<&str>,
    decide: impl FnOnce(&Reported, &Stamp) -> Result<Decision<Value>, WgError>,
) -> Result<Value, WgError> {
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| load_reported(session, scope, reference, attempt_id),
            decide,
            index_written,
        )?
        .reply)
}

/// `TaskService.claim`; `body` is the optional `ClaimRequest`.
pub fn task_claim(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    body: &Json,
) -> Result<Value, WgError> {
    let read = read_claim(body)?;
    app.require_permission(context, Permission::TaskClaim)?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(scope, &[]),
            |world: &TaskWorld, stamp| claims::claim_task(world, stamp, scope, reference, &read),
            index_written,
        )?
        .reply)
}

/// `TaskService.release`; `body` is the optional `ReasonBody`.
pub fn task_release(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    body: &Json,
) -> Result<Value, WgError> {
    let reason = read_reason(body)?;
    app.require_permission(context, Permission::TaskClaim)?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(scope, &[]),
            |world: &TaskWorld, stamp| {
                claims::release_task(world, stamp, scope, reference, reason.as_deref())
            },
            index_written,
        )?
        .reply)
}

/// `TaskService.complete`; `body` is the optional `CompleteBody`.
pub fn task_complete(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    body: &Json,
) -> Result<Value, WgError> {
    let read = read_complete(body)?;
    app.require_permission(context, Permission::TaskComplete)?;
    let scope = app.workspace_or_all(workspace)?;
    task_reported(
        app,
        context,
        scope,
        reference,
        read.attempt_id.as_deref(),
        |reported, stamp| claims::complete_task(reported, stamp, scope, reference, &read),
    )
}

/// `TaskService.report_result`; `body` is the `ResultReport`.
pub fn task_result(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    body: &Json,
) -> Result<Value, WgError> {
    let read = read_result(body)?;
    app.require_permission(context, Permission::TaskComplete)?;
    let scope = app.workspace_or_all(workspace)?;
    task_reported(
        app,
        context,
        scope,
        reference,
        read.attempt_id.as_deref(),
        |reported, stamp| claims::report_result(reported, stamp, scope, reference, &read),
    )
}

/// `TaskService.add_comment`: a `CommentOut`.
pub fn task_comment(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    body: &Json,
) -> Result<Value, WgError> {
    let read = read_comment(body)?;
    app.require_permission(context, Permission::TaskWrite)?;
    let scope = app.workspace_or_all(workspace)?;
    task_reported(
        app,
        context,
        scope,
        reference,
        read.attempt_id.as_deref(),
        |reported, stamp| {
            links::add_comment(
                &reported.world,
                reported.attempt.as_ref(),
                stamp,
                scope,
                reference,
                &read,
            )
        },
    )
}

/// `TaskService.report_progress`: a `CommentOut`.
pub fn task_progress(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    body: &Json,
) -> Result<Value, WgError> {
    let read = read_progress(body)?;
    app.require_permission(context, Permission::TaskWrite)?;
    let scope = app.workspace_or_all(workspace)?;
    task_reported(
        app,
        context,
        scope,
        reference,
        read.attempt_id.as_deref(),
        |reported, stamp| {
            links::report_progress(
                &reported.world,
                reported.attempt.as_ref(),
                stamp,
                scope,
                reference,
                &read,
            )
        },
    )
}

/// `TaskService.add_dependency`; `body` is the `DependencyBody`.
pub fn task_add_dependency(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    body: &Json,
) -> Result<Value, WgError> {
    let depends_on = read_dependency(body)?;
    task_dependency(app, context, workspace, reference, &depends_on, true)
}

/// `TaskService.remove_dependency`.
pub fn task_remove_dependency(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    depends_on: &str,
) -> Result<Value, WgError> {
    task_dependency(app, context, workspace, reference, depends_on, false)
}

fn task_dependency(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    depends_on: &str,
    add: bool,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskWrite)?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(scope, &[]),
            |world: &TaskWorld, stamp| {
                if add {
                    links::add_dependency(world, stamp, scope, reference, depends_on)
                } else {
                    links::remove_dependency(world, stamp, scope, reference, depends_on)
                }
            },
            index_written,
        )?
        .reply)
}

/// `TaskService.add_relation`: a `RelationOut`.
pub fn task_add_relation(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    body: &Json,
) -> Result<Value, WgError> {
    let read = read_relation(body)?;
    app.require_permission(context, Permission::TaskWrite)?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(scope, &[]),
            |world: &TaskWorld, stamp| links::add_relation(world, stamp, scope, reference, &read),
            index_written,
        )?
        .reply)
}

/// `TaskService.link_repository`; `body` is the `RepoBody`.
pub fn task_link_repository(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    body: &Json,
) -> Result<Value, WgError> {
    let read = read_repo(body)?;
    app.require_permission(context, Permission::TaskWrite)?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(scope, &[]),
            |world: &TaskWorld, stamp| {
                links::link_repository(world, stamp, scope, reference, &read)
            },
            index_written,
        )?
        .reply)
}

/// `TaskService.unlink_repository`.
pub fn task_unlink_repository(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    repository: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskWrite)?;
    let scope = app.workspace_or_all(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| session.task_world(scope, &[]),
            |world: &TaskWorld, stamp| {
                links::unlink_repository(world, stamp, scope, reference, repository)
            },
            index_written,
        )?
        .reply)
}

/// `AttemptService.list` for one task: a page of attempts, newest first.
pub fn task_attempts(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    limit: i64,
    offset: i64,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| {
            let world = session.task_world(Some(scope), &[])?;
            let task = resolve::task(&world.tasks, Some(scope), reference)?.id;
            session.attempts_page(scope, Some(task), &[], None, limit, offset)
        },
        |page| {
            Ok(reads::attempts_page_out(
                &page.items,
                page.total,
                page.limit,
                page.offset,
                page.next_offset,
            ))
        },
    )
}

/// The task world, the verification configurations of the task's workspace and the attempt
/// `attempt_id` names, if any (`None` when no such attempt exists).
fn load_reported(
    session: &mut Session<'_>,
    scope: Option<Id>,
    reference: &str,
    attempt_id: Option<&str>,
) -> Result<Reported, WgError> {
    let world = session.task_world(scope, &[])?;
    let workspace = resolve::task(&world.tasks, scope, reference)
        .map(|task| task.id)
        .ok()
        .and_then(|task| world.tasks.task(task).map(|found| found.workspace_id));
    let configs = match workspace {
        Some(id) => session.verification_configs(id)?,
        None => Vec::new(),
    };
    let attempt = match attempt_id.and_then(|text| parse_id(text).ok()) {
        Some(id) => session.attempt_by_id(id)?,
        None => None,
    };
    Ok(Reported {
        world,
        configs,
        attempt,
    })
}
