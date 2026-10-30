//! Comments, progress reports, dependencies, relations and repository links of a task
//! (`TaskService.add_comment`, `report_progress`, `add_dependency`, `remove_dependency`,
//! `add_relation`, `link_repository` and `unlink_repository`), as decisions over the task world.

use wg_json::Value;
use wg_model::entity::{AgentAttempt, TaskDependency, TaskRelation, TaskRepository};
use wg_model::event::{
    EventDraft, EventPayload, TaskDependencyChanged, TaskRelationAdded, TaskRepositoryLinked,
    TaskRepositoryUnlinked,
};
use wg_model::stamp::Stamp;
use wg_model::{Decision, RowKey, Task, Write};
use wg_registry::resolve;
use wg_rows::task_world_sql::TaskWorld;
use wg_tasks::deps::{DependencyAdd, check_add_dependency};
use wg_tasks::links::{
    RelationAdd, RepoLinkPlan, check_add_relation, plan_link_repository, plan_unlink_repository,
};
use wg_tasks::ownership::SYNC_ACTOR;
use wg_tasks::transition::{Applied, refresh_one};
use wg_types::{CommentKind, Id, Permission, WgError};
use wg_wire::dto::{comment, relation};

use super::bodies::{CommentRead, ProgressRead, RelationRead, RepoRead};
use super::claims::owned_attempt;
use wg_tasks::accept::comment_step;
use wg_tasks::comments::progress_data;
use wg_tasks::shape::task_out;

fn gone(task: Id) -> WgError {
    WgError::workgraph(format!("task {task} vanished from the slice"))
}

/// `TaskService._comment` and the reply of `add_comment`: a `CommentOut`.
fn comment_on(
    world: &TaskWorld,
    attempt: Option<&AgentAttempt>,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    read: &CommentRead,
) -> Result<Decision<Value>, WgError> {
    let task = resolve::task(&world.tasks, workspace, reference)?;
    let attempt_id = owned_attempt(task.id, read.attempt_id.as_deref(), attempt)?;
    let step = comment_step(
        stamp,
        stamp.ids(),
        task.id,
        task.workspace_id,
        read.kind,
        &read.body,
        &read.data,
        attempt_id,
        None,
    );
    let out = comment::from_model(&step.row);
    Ok(step.decision.with_reply(out))
}

/// `TaskService.add_comment`.
pub fn add_comment(
    world: &TaskWorld,
    attempt: Option<&AgentAttempt>,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    read: &CommentRead,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    comment_on(world, attempt, stamp, workspace, reference, read)
}

/// `TaskService.report_progress`: a progress comment, with the percent in its data.
pub fn report_progress(
    world: &TaskWorld,
    attempt: Option<&AgentAttempt>,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    read: &ProgressRead,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let comment = CommentRead {
        body: read.message.clone(),
        kind: CommentKind::Progress,
        data: progress_data(&read.data, read.percent),
        attempt_id: read.attempt_id.clone(),
    };
    comment_on(world, attempt, stamp, workspace, reference, &comment)
}

/// The task with its version raised by one (`task.version += 1`).
fn bumped(task: &Task, stamp: &Stamp) -> Task {
    Task {
        version: task.version + 1,
        updated_at: stamp.at,
        ..task.clone()
    }
}

/// The reply: the `TaskOut` of `task` in the state the decision leaves.
fn task_reply(world: &TaskWorld, applied: Applied, task: Id) -> Result<Decision<Value>, WgError> {
    let state = world.tasks.apply_all(applied.decision.writes.iter());
    let after = state.task(task).ok_or_else(|| gone(task))?;
    let out = task_out(&state, after, None);
    Ok(applied.decision.with_reply(out))
}

/// `TaskService.add_dependency`.
pub fn add_dependency(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    depends_on: &str,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let tasks = &world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let dep = resolve::task(tasks, Some(task.workspace_id), depends_on)?;
    let start = Applied {
        decision: Decision::new(()),
        ids: stamp.ids(),
        tasks: tasks.clone(),
    };
    let applied = match check_add_dependency(tasks, task.id, dep.id)? {
        DependencyAdd::Exists => start,
        DependencyAdd::Add => {
            let (row_id, ids) = start.ids.next();
            let (event_id, ids) = ids.next();
            let decision = Decision::new(())
                .write(Write::insert(TaskDependency {
                    id: row_id,
                    task_id: task.id,
                    depends_on_id: dep.id,
                    created_by: stamp.actor.to_string(),
                    created_at: stamp.at,
                }))
                .write(Write::update(bumped(task, stamp)))
                .event(EventDraft::new(
                    stamp,
                    event_id,
                    Some(task.workspace_id),
                    task.id,
                    EventPayload::TaskDependencyAdded(TaskDependencyChanged {
                        depends_on: dep.id,
                        depends_on_key: dep.key.clone(),
                    }),
                ));
            let changed = Applied {
                tasks: tasks.apply_all(decision.writes.iter()),
                decision,
                ids,
            };
            refresh_one(changed, &world.stacks, stamp, task.id, event_id)?
        }
    };
    task_reply(world, applied, task.id)
}

/// `TaskService.remove_dependency`.
pub fn remove_dependency(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    depends_on: &str,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let tasks = &world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let dep = resolve::task(tasks, Some(task.workspace_id), depends_on)?;
    let edge = tasks
        .dependencies_of(task.id)
        .find(|row| row.depends_on_id == dep.id);
    if let Some(row) = edge
        && row.created_by == SYNC_ACTOR
        && stamp.actor.to_string() != SYNC_ACTOR
    {
        return Err(WgError::permission_denied(format!(
            "the dependency of {} on {} comes from the tracker; remove it there",
            task.key, dep.key
        )));
    }
    let start = Applied {
        decision: Decision::new(()),
        ids: stamp.ids(),
        tasks: tasks.clone(),
    };
    let applied = match edge {
        None => start,
        Some(row) => {
            let (event_id, ids) = start.ids.next();
            let decision = Decision::new(())
                .write(Write::delete(RowKey::TaskDependency(row.id)))
                .write(Write::update(bumped(task, stamp)))
                .event(EventDraft::new(
                    stamp,
                    event_id,
                    Some(task.workspace_id),
                    task.id,
                    EventPayload::TaskDependencyRemoved(TaskDependencyChanged {
                        depends_on: dep.id,
                        depends_on_key: dep.key.clone(),
                    }),
                ));
            let changed = Applied {
                tasks: tasks.apply_all(decision.writes.iter()),
                decision,
                ids,
            };
            refresh_one(changed, &world.stacks, stamp, task.id, event_id)?
        }
    };
    task_reply(world, applied, task.id)
}

/// `TaskService.add_relation`: the `RelationOut` of the new (or existing) relation.
pub fn add_relation(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    read: &RelationRead,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let tasks = &world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let target = resolve::task(tasks, Some(task.workspace_id), &read.target)?;
    match check_add_relation(tasks, task.id, target.id, read.relation_type)? {
        RelationAdd::Exists(id) => {
            let row = tasks.relations.get(&id).ok_or_else(|| gone(task.id))?;
            Ok(Decision::new(relation::from_model(row)))
        }
        RelationAdd::Add => {
            let ([row_id, event_id], _) = stamp.ids().take::<2>();
            let row = TaskRelation {
                id: row_id,
                source_task_id: task.id,
                target_task_id: target.id,
                relation_type: read.relation_type,
                created_by: stamp.actor.to_string(),
                created_at: stamp.at,
            };
            let out = relation::from_model(&row);
            Ok(Decision::new(())
                .write(Write::insert(row))
                .event(EventDraft::new(
                    stamp,
                    event_id,
                    Some(task.workspace_id),
                    task.id,
                    EventPayload::TaskRelationAdded(TaskRelationAdded {
                        target: target.id,
                        target_key: target.key.clone(),
                        relation_type: read.relation_type,
                    }),
                ))
                .with_reply(out))
        }
    }
}

/// `TaskService.link_repository`.
pub fn link_repository(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    read: &RepoRead,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let tasks = &world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let repository =
        resolve::repository(&tasks.registry, task.workspace_id, &read.repository, false)?;
    let existing = tasks.task_repositories.get(&(task.id, repository.id));
    let (event_id, ids) = stamp.ids().next();
    let decision = match plan_link_repository(task, repository, read.access.as_deref(), existing)? {
        RepoLinkPlan::Insert { access } => Decision::new(())
            .write(Write::insert(TaskRepository {
                task_id: task.id,
                repository_id: repository.id,
                access: access.to_owned(),
                created_at: stamp.at,
            }))
            .write(Write::update(bumped(task, stamp)))
            .event(EventDraft::new(
                stamp,
                event_id,
                Some(task.workspace_id),
                task.id,
                EventPayload::TaskRepositoryLinked(TaskRepositoryLinked {
                    repository_id: repository.id,
                    repository: repository.name.clone(),
                    access: access.to_owned(),
                }),
            )),
        RepoLinkPlan::Update { access } => {
            let link = existing.ok_or_else(|| gone(task.id))?;
            Decision::new(()).write(Write::update(TaskRepository {
                access: access.to_owned(),
                ..link.clone()
            }))
        }
        RepoLinkPlan::Unchanged => Decision::new(()),
    };
    task_reply(
        world,
        Applied {
            decision,
            ids,
            tasks: tasks.clone(),
        },
        task.id,
    )
}

/// `TaskService.unlink_repository`.
pub fn unlink_repository(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    repository: &str,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let tasks = &world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let repo = resolve::repository(&tasks.registry, task.workspace_id, repository, true)?;
    let linked = tasks.task_repositories.contains_key(&(task.id, repo.id));
    let (event_id, ids) = stamp.ids().next();
    let decision = match plan_unlink_repository(repo, linked) {
        Some(_) => Decision::new(())
            .write(Write::delete(RowKey::TaskRepository {
                task_id: task.id,
                repository_id: repo.id,
            }))
            .write(Write::update(bumped(task, stamp)))
            .event(EventDraft::new(
                stamp,
                event_id,
                Some(task.workspace_id),
                task.id,
                EventPayload::TaskRepositoryUnlinked(TaskRepositoryUnlinked {
                    repository_id: repo.id,
                    repository: repo.name.clone(),
                }),
            )),
        None => Decision::new(()),
    };
    task_reply(
        world,
        Applied {
            decision,
            ids,
            tasks: tasks.clone(),
        },
        task.id,
    )
}
