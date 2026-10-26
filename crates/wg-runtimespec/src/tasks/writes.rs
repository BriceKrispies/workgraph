//! The task writes: create, update, archive and promote (`TaskService.create`, `update`,
//! `archive` and `promote_ready_report`), as decisions over the task world.
//!
//! Each function checks the permission, resolves the references the request names, asks the
//! `wg-tasks` plans for the rows and events, applies the status changes that follow, and returns
//! a [`Decision`] whose reply is the output value the command prints.

use serde_json::{Value as Json, json};
use wg_jobs::queue::{NewJob, enqueue};
use wg_json::Value;
use wg_model::entity::{Task, TaskFile, TaskProject, TaskTag, Workspace};
use wg_model::event::{EventDraft, EventPayload, TaskKeyed};
use wg_model::slice::QueueSlice;
use wg_model::stamp::{IdSeq, Stamp, derive_id};
use wg_model::{Decision, RowKey, Write};
use wg_registry::resolve;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::archive::{ArchiveChange, archive_plan};
use wg_tasks::create::{Allocation, NewTask, RepositoryLink, create_plan};
use wg_tasks::promote::{PromoteScope, promote_ready as promotable, promoted_by_project};
use wg_tasks::transition::{Applied, Change, transition};
use wg_tasks::update::{TaskPatch, update_plan};
use wg_types::{ErrorCode, Id, Permission, TaskKey, TaskStatus, WgError};
use wg_wire::convention::{dict, text};

use super::out::{echo_naive, task_out};
use super::requests::{CreateRead, UpdateRead};

/// Whether a text change queues reclassification and re-embedding (`llm.auto_classify` and
/// `llm.auto_embed`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Enrichment {
    pub classify: bool,
    pub embed: bool,
}

/// `config.llm.auto_classify` and `auto_embed` (both default on) from the configuration.
#[must_use]
pub fn enrichment_from(config: &serde_json::Map<String, Json>) -> Enrichment {
    let llm = config.get("llm").and_then(Json::as_object);
    let flag = |key: &str| {
        llm.and_then(|section| section.get(key))
            .and_then(Json::as_bool)
            .unwrap_or(true)
    };
    Enrichment {
        classify: flag("auto_classify"),
        embed: flag("auto_embed"),
    }
}

/// A stamp with its own ids, for a helper that always draws its first id from slot 0.
fn substamp(stamp: &Stamp, salt: u32) -> Stamp {
    let a = derive_id(&stamp.entropy, 1_000_000 + salt);
    let b = derive_id(&stamp.entropy, 2_000_000 + salt);
    let entropy: Vec<u8> = a.as_bytes().iter().chain(b.as_bytes()).copied().collect();
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&entropy);
    Stamp {
        entropy: bytes,
        ..stamp.clone()
    }
}

/// `TaskService._enqueue_enrichment`: the classify and embed jobs of a material change. `queue`
/// holds the jobs already stored for the task: an enqueue under an idempotency key that a stored
/// job holds gives that job back and queues nothing (`JobQueue.enqueue`).
fn enrichment_jobs(
    stamp: &Stamp,
    task: &Task,
    caused_by: Id,
    enrich: Enrichment,
    queue: &QueueSlice,
) -> Decision<()> {
    let mut decision = Decision::new(());
    let mut salt = 0;
    let mut add = |decision: Decision<()>, kind: &str, payload: Json, key: String| {
        let mut job = NewJob::new(kind);
        job.payload = payload.as_object().cloned().unwrap_or_default();
        job.workspace_id = Some(task.workspace_id);
        job.task_id = Some(task.id);
        job.idempotency_key = Some(key);
        job.resource_keys = vec!["gpu".to_owned()];
        job.max_attempts = 4;
        job.backoff_base_seconds = 15.0;
        job.correlation_id = Some(stamp.correlation.clone());
        job.causation_id = Some(caused_by);
        job.created_by = stamp.actor.to_string();
        salt += 1;
        let queued = enqueue(queue, &job, &substamp(stamp, salt));
        queued
            .writes
            .iter()
            .fold(decision, |acc, write| acc.write(write.clone()))
    };
    if enrich.classify {
        decision = add(
            decision,
            "classify_task",
            json!({"task_id": task.id.to_string(), "reason": "material_change"}),
            format!("classify:{}:{}", task.id, task.description_hash),
        );
    }
    if enrich.embed {
        decision = add(
            decision,
            "embed_task",
            json!({"task_id": task.id.to_string()}),
            format!("embed:{}:{}", task.id, task.description_hash),
        );
    }
    decision
}

/// Appends `other`'s writes, events and effects to `main`.
fn absorb(main: Decision<()>, other: Decision<()>) -> Decision<()> {
    main.then(other)
}

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

/// The rows that replace a task's tags: the old ones that are gone, the new ones that are not
/// there yet.
pub(crate) fn tag_writes(task: &Task, before: &[String], after: &[String]) -> Vec<Write> {
    let removed = before.iter().filter(|tag| !after.contains(tag)).map(|tag| {
        Write::delete(RowKey::TaskTag {
            task_id: task.id,
            tag: tag.clone(),
        })
    });
    let added = after.iter().filter(|tag| !before.contains(tag)).map(|tag| {
        Write::insert(TaskTag {
            task_id: task.id,
            tag: tag.clone(),
        })
    });
    removed.chain(added).collect()
}

fn file_writes(task: &Task, before: usize, rows: Vec<TaskFile>) -> Vec<Write> {
    (0..before)
        .map(|position| {
            Write::delete(RowKey::TaskFile {
                task_id: task.id,
                position: i64::try_from(position).unwrap_or(i64::MAX),
            })
        })
        .chain(rows.into_iter().map(Write::insert))
        .collect()
}

fn project_writes(task: &Task, before: &[Id], after: &[Id]) -> Vec<Write> {
    let removed = before.iter().filter(|id| !after.contains(id)).map(|id| {
        Write::delete(RowKey::TaskProject {
            task_id: task.id,
            project_id: *id,
        })
    });
    let added = after.iter().filter(|id| !before.contains(id)).map(|id| {
        Write::insert(TaskProject {
            task_id: task.id,
            project_id: *id,
        })
    });
    removed.chain(added).collect()
}

/// `TaskService.update`: the update of one task, resolved from `reference`. `queue` holds the jobs
/// stored for that task (see [`enrichment_jobs`]).
pub fn update_task(
    world: &TaskWorld,
    queue: &QueueSlice,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    read: UpdateRead,
    enrich: Enrichment,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let tasks = &world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let projects = read
        .projects
        .as_ref()
        .map(|refs| {
            refs.iter()
                .map(|reference| {
                    resolve::project(&tasks.registry, task.workspace_id, reference).map(|p| p.id)
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let parent = match (read.clear_parent && task.parent_id.is_some(), &read.parent) {
        (false, Some(reference)) => {
            Some(resolve::task(tasks, Some(task.workspace_id), reference)?.id)
        }
        _ => None,
    };
    let patch = TaskPatch {
        expected_version: read.expected_version,
        title: read.title,
        description: read.description,
        acceptance_criteria: read.acceptance_criteria,
        assignee: read.assignee,
        assigned_agent: read.assigned_agent,
        auto_execute: read.auto_execute,
        due_at: read.due_at,
        scheduled_at: read.scheduled_at,
        priority: read.priority,
        estimated_effort: read.estimated_effort,
        metadata: read.metadata,
        tags: read.tags,
        add_tags: read.add_tags,
        remove_tags: read.remove_tags,
        files: read.files,
        add_files: read.add_files,
        projects,
        parent,
        clear_parent: read.clear_parent,
        status: read.status,
        reason: read.reason,
    };
    let plan = update_plan(stamp, tasks, task, &patch)?;

    let before_tags: Vec<String> = tasks.tags_of(task.id).map(str::to_owned).collect();
    let before_files = tasks.files_of(task.id).count();
    let before_projects: Vec<Id> = tasks
        .projects_of(task.id)
        .map(|row| row.project_id)
        .collect();

    let mut decision = Decision::new(());
    if plan.task != *task {
        decision = decision.write(Write::update(plan.task.clone()));
    }
    let tag_order = plan.tags.clone();
    if let Some(after) = &plan.tags {
        for write in tag_writes(task, &before_tags, after) {
            decision = decision.write(write);
        }
    }
    if let Some(rows) = plan.files {
        for write in file_writes(task, before_files, rows) {
            decision = decision.write(write);
        }
    }
    if let Some(after) = &plan.projects {
        for write in project_writes(task, &before_projects, after) {
            decision = decision.write(write);
        }
    }
    let ids = stamp.ids();
    let (event_id, ids) = ids.next();
    let event_id = plan.event.as_ref().map(|_| event_id);
    if let (Some(payload), Some(id)) = (plan.event, event_id) {
        decision = decision.event(EventDraft::new(
            stamp,
            id,
            Some(task.workspace_id),
            task.id,
            EventPayload::TaskUpdated(payload),
        ));
    }
    let mut state = tasks.apply_all(decision.writes.iter());
    if let Some(target) = plan.status {
        let moved: Applied = transition(
            &state,
            &world.stacks,
            stamp,
            ids,
            task.id,
            target,
            Change {
                reason: patch.reason.as_deref(),
                caused_by: event_id,
                ..Change::default()
            },
        )?;
        state = moved.tasks;
        decision = absorb(decision, moved.decision);
    }
    let after = state
        .task(task.id)
        .ok_or_else(|| WgError::new(ErrorCode::WorkgraphError, "the updated task vanished"))?;
    if let (true, true, Some(cause)) = (
        plan.material_change,
        enrich != Enrichment::default(),
        event_id,
    ) {
        decision = absorb(
            decision,
            enrichment_jobs(stamp, after, cause, enrich, queue),
        );
    }
    let out = task_out(&state, after, tag_order.as_deref());
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
