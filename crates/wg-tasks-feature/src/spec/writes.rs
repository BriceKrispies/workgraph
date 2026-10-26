//! The task writes: create, update, archive and promote (`TaskService.create`, `update`,
//! `archive` and `promote_ready_report`), as decisions over the task world.
//!
//! Each function checks the permission, resolves the references the request names, asks the
//! `wg-tasks` plans for the rows and events, applies the status changes that follow, and returns
//! a [`Decision`] whose reply is the output value the command prints.

use wg_json::Value;
use wg_model::entity::Workspace;
use wg_model::event::{EventDraft, EventPayload, TaskKeyed};
use wg_model::slice::QueueSlice;
use wg_model::stamp::{IdSeq, Stamp};
use wg_model::{Decision, Write};
use wg_registry::resolve;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::archive::{ArchiveChange, archive_plan};
use wg_tasks::create::{Allocation, NewTask, RepositoryLink, create_plan};
use wg_tasks::edit::*;
use wg_tasks::promote::{PromoteScope, promote_ready as promotable, promoted_by_project};
use wg_tasks::transition::{Change, transition};
use wg_types::{ErrorCode, Id, Permission, TaskKey, TaskStatus, WgError};
use wg_wire::convention::{dict, text};

use super::requests::CreateRead;
use wg_tasks::shape::{echo_naive, task_out};

/// `TaskService.create`.
pub fn create_task(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Id,
    read: CreateRead,
    enrich: Enrichment,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let tasks = &world.tasks;
    let space = tasks.registry.workspace(workspace).ok_or_else(|| {
        WgError::new(
            ErrorCode::NotFound,
            format!("workspace {workspace} not found"),
        )
    })?;
    let key_taken = read
        .key
        .as_deref()
        .and_then(|key| TaskKey::parse(key).ok())
        .is_some_and(|key| tasks.task_by_key(space.id, &key).is_some());
    let parent_id = read
        .parent
        .as_deref()
        .map(|reference| resolve::task(tasks, Some(space.id), reference))
        .transpose()?
        .map(|parent| parent.id);
    let projects = read
        .projects
        .iter()
        .map(|reference| resolve::project(&tasks.registry, space.id, reference).map(|p| p.id))
        .collect::<Result<Vec<_>, _>>()?;
    let repositories = read
        .repositories
        .iter()
        .map(|reference| {
            resolve::repository(&tasks.registry, space.id, reference, false).map(|repository| {
                RepositoryLink {
                    repository_id: repository.id,
                    access: repository.access_mode.as_str().to_owned(),
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let depends_on = read
        .depends_on
        .iter()
        .map(|reference| resolve::task(tasks, Some(space.id), reference).map(|task| task.id))
        .collect::<Result<Vec<_>, _>>()?;

    let (naive_due, naive_scheduled) = (read.due_naive, read.scheduled_naive);
    let plan = create_plan(
        stamp,
        Allocation {
            workspace_id: space.id,
            key_prefix: &space.key_prefix,
            number: space.next_task_number,
            key_taken,
        },
        NewTask {
            key: read.key,
            title: read.title,
            description: read.description,
            acceptance_criteria: read.acceptance_criteria,
            status: read.status,
            priority: read.priority,
            parent_id,
            assignee: read.assignee,
            assigned_agent: read.assigned_agent,
            auto_execute: read.auto_execute,
            due_at: read.due_at,
            scheduled_at: read.scheduled_at,
            estimated_effort: read.estimated_effort,
            metadata: read.metadata,
            tags: read.tags,
            files: read.files,
            projects,
            repositories,
            depends_on,
        },
    )?;

    let task_id = plan.task.id;
    // The plan drew the task id and one id per dependency; events continue after them.
    let ids = (0..=plan.dependencies.len()).fold(stamp.ids(), |ids: IdSeq, _| ids.next().1);
    let (created_id, ids) = ids.next();
    let tag_order: Vec<String> = plan.tags.iter().map(|row| row.tag.clone()).collect();

    let mut decision = Decision::new(()).write(Write::update(Workspace {
        next_task_number: space.next_task_number + 1,
        updated_at: stamp.at,
        ..space.clone()
    }));
    decision = decision.write(Write::insert(plan.task.clone()));
    for row in plan.tags {
        decision = decision.write(Write::insert(row));
    }
    for row in plan.files {
        decision = decision.write(Write::insert(row));
    }
    for row in plan.projects {
        decision = decision.write(Write::insert(row));
    }
    for row in plan.repositories {
        decision = decision.write(Write::insert(row));
    }
    for row in plan.dependencies {
        decision = decision.write(Write::insert(row));
    }
    decision = decision.event(EventDraft::new(
        stamp,
        created_id,
        Some(space.id),
        task_id,
        EventPayload::TaskCreated(plan.event),
    ));

    let mut state = tasks.apply_all(decision.writes.iter());
    if plan.ready {
        let moved = transition(
            &state,
            &world.stacks,
            stamp,
            ids,
            task_id,
            TaskStatus::Ready,
            Change {
                reason: Some("created ready"),
                caused_by: Some(created_id),
                ..Change::default()
            },
        )?;
        state = moved.tasks;
        decision = absorb(decision, moved.decision);
    }
    let created = state
        .task(task_id)
        .ok_or_else(|| WgError::new(ErrorCode::WorkgraphError, "the created task vanished"))?;
    // The task id was drawn just now, so no job can carry its idempotency keys yet.
    decision = absorb(
        decision,
        enrichment_jobs(stamp, created, created_id, enrich, &QueueSlice::default()),
    );
    let out = task_out(&state, created, Some(&tag_order));
    // A datetime the request gave without an offset is echoed naive, as Python does.
    let out = echo_naive(out, "due_at", created.due_at.filter(|_| naive_due));
    let out = echo_naive(
        out,
        "scheduled_at",
        created.scheduled_at.filter(|_| naive_scheduled),
    );
    Ok(decision.with_reply(out))
}

/// `TaskService.archive`.
pub fn archive_task(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    archived: bool,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let tasks = &world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let plan = archive_plan(stamp, tasks, task, archived)?;
    let decision = match plan.change {
        ArchiveChange::Unchanged => Decision::new(()),
        change => {
            let mut next = plan.task.clone();
            next.updated_at = stamp.at;
            let (id, _) = stamp.ids().next();
            let key = TaskKeyed {
                key: next.key.clone(),
            };
            let payload = if change == ArchiveChange::Archived {
                EventPayload::TaskArchived(key)
            } else {
                EventPayload::TaskUnarchived(key)
            };
            Decision::new(())
                .write(Write::update(next))
                .event(EventDraft::new(
                    stamp,
                    id,
                    Some(task.workspace_id),
                    task.id,
                    payload,
                ))
        }
    };
    let state = tasks.apply_all(decision.writes.iter());
    let after = state.task(task.id).unwrap_or(task);
    let out = task_out(&state, after, None);
    Ok(decision.with_reply(out))
}

/// `TaskService.promote_ready_report` plus the `dry_run` flag the command adds: moves the
/// backlog tasks whose dependencies are accepted to ready. With `dry_run` the decision changes
/// nothing.
pub fn promote_ready(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Id,
    tags: &[String],
    project: Option<&str>,
    dry_run: bool,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let tasks = &world.tasks;
    let project_id = project
        .filter(|reference| !reference.is_empty())
        .map(|reference| resolve::project(&tasks.registry, workspace, reference))
        .transpose()?
        .map(|found| found.id);
    let moved = promotable(
        tasks,
        &world.stacks,
        workspace,
        PromoteScope {
            tags,
            project: project_id,
        },
    );
    let mut decision = Decision::new(());
    if !dry_run {
        let mut state = tasks.clone();
        let mut ids = stamp.ids();
        for task in &moved {
            let applied = transition(
                &state,
                &world.stacks,
                stamp,
                ids,
                task.id,
                TaskStatus::Ready,
                Change {
                    reason: Some("dependencies accepted"),
                    ..Change::default()
                },
            )?;
            state = applied.tasks;
            ids = applied.ids;
            decision = absorb(decision, applied.decision);
        }
    }
    let keys: Vec<Value> = moved
        .iter()
        .map(|task| text(&task.key.to_string()))
        .collect();
    let counts: Vec<(wg_json::PyStr, Value)> = promoted_by_project(tasks, &moved)
        .into_iter()
        .map(|(slug, count)| {
            (
                wg_json::PyStr::from(slug.as_str()),
                Value::Int(i128::try_from(count).unwrap_or(i128::MAX)),
            )
        })
        .collect();
    let out = dict(vec![
        ("promoted", Value::List(keys)),
        ("by_project", Value::Dict(counts)),
        ("dry_run", Value::Bool(dry_run)),
    ]);
    Ok(decision.with_reply(out))
}
