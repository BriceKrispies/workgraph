//! The task entry points of the application (IF-05): the services behind `task create`, `list`,
//! `show`, `update`, `bulk-update`, `archive`, `history`, `graph`, `ready` and `promote-ready`.
//!
//! Each one resolves the workspace, loads the task world in the transaction of the call, hands it
//! to the pure function of `wg_runtimespec::tasks` and applies what it returns. Writes keep the
//! full-text index in step in the same transaction.

use std::collections::BTreeSet;

use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_model::slice::QueueSlice;
use wg_model::{Decision, Row, Stamp, Write};
use wg_registry::resolve;
use wg_runtimespec::RequestContext;
use wg_runtimespec::tasks::bodies::{
    read_claim, read_comment, read_complete, read_dependency, read_progress, read_reason,
    read_relation, read_repo, read_result,
};
use wg_runtimespec::tasks::claims::{self, Reported};
use wg_runtimespec::tasks::links;
use wg_runtimespec::tasks::out::{bulk_result_out, key_of};
use wg_runtimespec::tasks::reads::{self, GraphRequest, Shown};
use wg_runtimespec::tasks::requests::{
    read_create, read_filter, read_update, validate_bulk, validate_create, validate_filter,
    validate_update,
};
use wg_runtimespec::tasks::writes;
use wg_store::{EventQuery, Session, TaskWorld};
use wg_tasks::bulk::bulk_results;
use wg_types::{ErrorCode, Id, Permission, WgError, parse_id};

use crate::app::App;

/// Keeps `task_fts` in step with the tasks a decision wrote (`index_task`).
pub fn index_written<R>(
    session: &mut Session<'_>,
    decision: &wg_model::Decision<R>,
) -> Result<(), WgError> {
    let written: BTreeSet<Id> = decision
        .writes
        .iter()
        .filter_map(|write| match write {
            Write::Insert(Row::Task(task))
            | Write::Update {
                row: Row::Task(task),
                ..
            } => Some(task.id),
            Write::Insert(Row::TaskComment(comment)) => Some(comment.task_id),
            _ => None,
        })
        .collect();
    written
        .into_iter()
        .try_for_each(|task_id| session.index_task(task_id))
}

impl App {
    /// `TaskService._workspace_id`: the named workspace, else the default one.
    pub fn workspace_or_default(&mut self, workspace: Option<&str>) -> Result<Id, WgError> {
        match workspace {
            Some(reference) => self.workspace_id(reference),
            None => self.default_workspace_id(),
        }
    }

    /// The workspace a task reference is looked up in: the named one, else every workspace.
    pub fn workspace_or_all(&mut self, workspace: Option<&str>) -> Result<Option<Id>, WgError> {
        workspace
            .map(|reference| self.workspace_id(reference))
            .transpose()
    }

    /// A read-permission check against a fresh stamp.
    pub fn require_read(&mut self, context: &RequestContext) -> Result<(), WgError> {
        self.runtime
            .stamps
            .stamp(context)?
            .require(&[Permission::TaskRead])
    }

    /// A permission check against a fresh stamp, before anything is looked up.
    pub(crate) fn require_permission(
        &mut self,
        context: &RequestContext,
        permission: Permission,
    ) -> Result<(), WgError> {
        self.runtime.stamps.stamp(context)?.require(&[permission])
    }

    /// `TaskService.create`.
    pub fn task_create(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        body: &Json,
    ) -> Result<Value, WgError> {
        let read = read_create(&validate_create(body)?)?;
        let enrich = writes::enrichment_from(&self.config()?);
        let scope = self.workspace_or_default(workspace)?;
        Ok(self
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let dump = validate_update(body)?;
        let given = body.as_object().cloned().unwrap_or_default();
        let enrich = writes::enrichment_from(&self.config()?);
        self.update_checked(context, workspace, reference, &dump, &given, enrich)
    }

    /// `TaskService.update(..., enrich=False)`: a text change that queues no reclassification or
    /// re-embedding (the words changed, not the task).
    pub(crate) fn task_update_plain(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let dump = validate_update(body)?;
        let given = body.as_object().cloned().unwrap_or_default();
        let enrich = writes::Enrichment::default();
        self.update_checked(context, workspace, reference, &dump, &given, enrich)
    }

    fn update_checked(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        dump: &Map<String, Json>,
        given: &Map<String, Json>,
        enrich: writes::Enrichment,
    ) -> Result<Value, WgError> {
        let read = read_update(dump, given)?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
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
                    writes::update_task(world, queue, stamp, scope, reference, read, enrich)
                },
                index_written,
            )?
            .reply)
    }

    /// `TaskService.bulk_update`: one update applied to many tasks, each in its own transaction.
    pub fn task_bulk_update(
        &mut self,
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
        let enrich = writes::enrichment_from(&self.config()?);
        let outcomes = refs
            .into_iter()
            .map(|reference| {
                let mut shared = update.clone();
                shared.insert("expected_version".to_owned(), Json::Null);
                let done = self
                    .update_checked(context, workspace, &reference, &shared, &given, enrich)
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        archived: bool,
    ) -> Result<Value, WgError> {
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        body: &Json,
    ) -> Result<Value, WgError> {
        let filter = read_filter(&validate_filter(body)?)?;
        self.require_read(context)?;
        let scope = self.workspace_or_default(workspace)?;
        let expression = reads::filter_match(&filter);
        self.runtime.query(
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        project: Option<&str>,
        repository: Option<&str>,
        agent: Option<&str>,
        limit: i64,
    ) -> Result<Value, WgError> {
        self.require_read(context)?;
        let scope = self.workspace_or_default(workspace)?;
        self.runtime.query(
            |session| session.task_world(Some(scope), &[]),
            |world| reads::ready(world, scope, project, repository, agent, limit),
        )
    }

    /// `TaskService.detail` for each reference.
    pub fn task_show(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        refs: &[String],
    ) -> Result<Shown, WgError> {
        self.require_read(context)?;
        let scope = self.workspace_or_all(workspace)?;
        self.runtime.query(
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        request: &GraphRequest<'_>,
    ) -> Result<Value, WgError> {
        self.require_read(context)?;
        let scope = self.workspace_or_default(workspace)?;
        self.runtime.query(
            |session| session.task_world(Some(scope), &[]),
            |world| reads::graph(world, scope, request),
        )
    }

    /// `HistoryService.task_history`: the events of a task and of its attempts, oldest first.
    pub fn task_history(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        event_types: &[String],
    ) -> Result<Value, WgError> {
        self.task_history_limited(context, workspace, reference, event_types, 500)
    }

    /// [`task_history`](Self::task_history) cut to the first `limit` events.
    pub fn task_history_limited(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        event_types: &[String],
        limit: i64,
    ) -> Result<Value, WgError> {
        self.require_read(context)?;
        let scope = self.workspace_or_all(workspace)?;
        self.runtime.query(
            |session| {
                let world = session.task_world(scope, &[])?;
                let task = reads::resolve_task_id(&world, scope, reference)?;
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
            |events| Ok(Value::List(events.iter().map(reads::event_out).collect())),
        )
    }

    /// `TaskService.promote_ready_report`, with the `dry_run` flag added.
    pub fn task_promote_ready(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        tags: &[String],
        project: Option<&str>,
        dry_run: bool,
    ) -> Result<Value, WgError> {
        let scope = self.workspace_or_default(workspace)?;
        if dry_run {
            let stamp = self.runtime.stamps.stamp(context)?;
            return self.runtime.query(
                |session| session.task_world(Some(scope), &[]),
                |world| {
                    writes::promote_ready(world, &stamp, scope, tags, project, true)
                        .map(|decision| decision.reply)
                },
            );
        }
        Ok(self
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
        &mut self,
        context: &RequestContext,
        scope: Option<Id>,
        reference: &str,
        attempt_id: Option<&str>,
        decide: impl FnOnce(&Reported, &Stamp) -> Result<Decision<Value>, WgError>,
    ) -> Result<Value, WgError> {
        Ok(self
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let read = read_claim(body)?;
        self.require_permission(context, Permission::TaskClaim)?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
            .runtime
            .decide_then(
                context,
                |session| session.task_world(scope, &[]),
                |world: &TaskWorld, stamp| {
                    claims::claim_task(world, stamp, scope, reference, &read)
                },
                index_written,
            )?
            .reply)
    }

    /// `TaskService.release`; `body` is the optional `ReasonBody`.
    pub fn task_release(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let reason = read_reason(body)?;
        self.require_permission(context, Permission::TaskClaim)?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let read = read_complete(body)?;
        self.require_permission(context, Permission::TaskComplete)?;
        let scope = self.workspace_or_all(workspace)?;
        self.task_reported(
            context,
            scope,
            reference,
            read.attempt_id.as_deref(),
            |reported, stamp| claims::complete_task(reported, stamp, scope, reference, &read),
        )
    }

    /// `TaskService.report_result`; `body` is the `ResultReport`.
    pub fn task_result(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let read = read_result(body)?;
        self.require_permission(context, Permission::TaskComplete)?;
        let scope = self.workspace_or_all(workspace)?;
        self.task_reported(
            context,
            scope,
            reference,
            read.attempt_id.as_deref(),
            |reported, stamp| claims::report_result(reported, stamp, scope, reference, &read),
        )
    }

    /// `TaskService.add_comment`: a `CommentOut`.
    pub fn task_comment(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let read = read_comment(body)?;
        self.require_permission(context, Permission::TaskWrite)?;
        let scope = self.workspace_or_all(workspace)?;
        self.task_reported(
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let read = read_progress(body)?;
        self.require_permission(context, Permission::TaskWrite)?;
        let scope = self.workspace_or_all(workspace)?;
        self.task_reported(
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let depends_on = read_dependency(body)?;
        self.task_dependency(context, workspace, reference, &depends_on, true)
    }

    /// `TaskService.remove_dependency`.
    pub fn task_remove_dependency(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        depends_on: &str,
    ) -> Result<Value, WgError> {
        self.task_dependency(context, workspace, reference, depends_on, false)
    }

    fn task_dependency(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        depends_on: &str,
        add: bool,
    ) -> Result<Value, WgError> {
        self.require_permission(context, Permission::TaskWrite)?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let read = read_relation(body)?;
        self.require_permission(context, Permission::TaskWrite)?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
            .runtime
            .decide_then(
                context,
                |session| session.task_world(scope, &[]),
                |world: &TaskWorld, stamp| {
                    links::add_relation(world, stamp, scope, reference, &read)
                },
                index_written,
            )?
            .reply)
    }

    /// `TaskService.link_repository`; `body` is the `RepoBody`.
    pub fn task_link_repository(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let read = read_repo(body)?;
        self.require_permission(context, Permission::TaskWrite)?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        repository: &str,
    ) -> Result<Value, WgError> {
        self.require_permission(context, Permission::TaskWrite)?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
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
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Value, WgError> {
        self.require_read(context)?;
        let scope = self.workspace_or_default(workspace)?;
        self.runtime.query(
            |session| {
                let world = session.task_world(Some(scope), &[])?;
                let task = reads::resolve_task_id(&world, Some(scope), reference)?;
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
    let workspace = reads::resolve_task_id(&world, scope, reference)
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
