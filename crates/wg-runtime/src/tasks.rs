//! The task entry points of the application (IF-05): the services behind `task create`, `list`,
//! `show`, `update`, `bulk-update`, `archive`, `history`, `graph`, `ready` and `promote-ready`.
//!
//! Each one resolves the workspace, loads the task world in the transaction of the call, hands it
//! to the pure function of `wg_runtimespec::tasks` and applies what it returns. Writes keep the
//! full-text index in step in the same transaction.

use std::collections::BTreeSet;

use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_model::{Row, Write};
use wg_runtimespec::RequestContext;
use wg_runtimespec::tasks::out::{bulk_result_out, key_of};
use wg_runtimespec::tasks::reads::{self, GraphRequest, Shown};
use wg_runtimespec::tasks::requests::{
    read_create, read_filter, read_update, validate_bulk, validate_create, validate_filter,
    validate_update,
};
use wg_runtimespec::tasks::writes;
use wg_store::{EventQuery, Session, TaskWorld};
use wg_tasks::bulk::bulk_results;
use wg_types::{ErrorCode, Id, Permission, WgError};

use crate::app::App;

/// Keeps `task_fts` in step with the tasks a decision wrote (`index_task`).
pub(crate) fn index_written<R>(
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
            _ => None,
        })
        .collect();
    written
        .into_iter()
        .try_for_each(|task_id| session.index_task(task_id))
}

impl App {
    /// `TaskService._workspace_id`: the named workspace, else the default one.
    pub(crate) fn workspace_or_default(&mut self, workspace: Option<&str>) -> Result<Id, WgError> {
        match workspace {
            Some(reference) => self.workspace_id(reference),
            None => self.default_workspace_id(),
        }
    }

    /// The workspace a task reference is looked up in: the named one, else every workspace.
    pub(crate) fn workspace_or_all(
        &mut self,
        workspace: Option<&str>,
    ) -> Result<Option<Id>, WgError> {
        workspace
            .map(|reference| self.workspace_id(reference))
            .transpose()
    }

    /// A read-permission check against a fresh stamp.
    pub(crate) fn require_read(&mut self, context: &RequestContext) -> Result<(), WgError> {
        self.runtime
            .stamps
            .stamp(context)?
            .require(&[Permission::TaskRead])
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
        self.update_checked(context, workspace, reference, &dump, &given)
    }

    fn update_checked(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        dump: &Map<String, Json>,
        given: &Map<String, Json>,
    ) -> Result<Value, WgError> {
        let read = read_update(dump, given)?;
        let enrich = writes::enrichment_from(&self.config()?);
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
            .runtime
            .decide_then(
                context,
                |session| session.task_world(scope, &[]),
                |world: &TaskWorld, stamp| {
                    writes::update_task(world, stamp, scope, reference, read, enrich)
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
        let outcomes = refs
            .into_iter()
            .map(|reference| {
                let mut shared = update.clone();
                shared.insert("expected_version".to_owned(), Json::Null);
                let done = self
                    .update_checked(context, workspace, &reference, &shared, &given)
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
                    limit: 500,
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
}
