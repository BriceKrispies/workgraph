//! Creating a task. Mirrors `_create_in_session` in `core/services/tasks.py`: the key, the task
//! row, its tags, files, projects, repositories and dependencies, and the `task.created` payload.
//! The shell reads the workspace's next number, writes the rows and event it is given, and moves
//! the task to ready when `ready` is set (that transition goes through the dependency gate).

use std::collections::BTreeSet;

use rpds::VectorSync;
use wg_model::entity::{JsonObject, Task, TaskDependency, TaskProject, TaskRepository, TaskTag};
use wg_model::event::TaskCreated;
use wg_model::stamp::Stamp;
use wg_types::{Id, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError};

use crate::content::content_hash;
use crate::files::{FileInput, file_rows};
use crate::ownership::check_tracker_key;

/// A repository a task is linked to, with the access the link grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryLink {
    pub repository_id: Id,
    pub access: String,
}

/// The validated input of a create, with references already resolved to ids by the shell.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTask {
    /// A tracker key (`ABC-123`); only the tracker sync may set one.
    pub key: Option<String>,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    /// The requested status. `ready` is applied after the insert; any other value leaves the
    /// task in backlog.
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub parent_id: Option<Id>,
    pub assignee: Option<String>,
    pub assigned_agent: Option<String>,
    pub auto_execute: bool,
    pub due_at: Option<Timestamp>,
    pub scheduled_at: Option<Timestamp>,
    pub estimated_effort: Option<JsonObject>,
    pub metadata: JsonObject,
    pub tags: Vec<String>,
    pub files: Vec<FileInput>,
    pub projects: Vec<Id>,
    pub repositories: Vec<RepositoryLink>,
    pub depends_on: Vec<Id>,
}

/// Where a new task goes in its workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allocation<'a> {
    pub workspace_id: Id,
    /// The workspace's key prefix (`WG`).
    pub key_prefix: &'a str,
    /// The workspace's `next_task_number`; the shell advances the counter.
    pub number: i64,
    /// Whether a task in this workspace already has the requested tracker key.
    pub key_taken: bool,
}

/// The rows and event that a create writes, in the order the shell applies them.
#[derive(Debug, Clone, PartialEq)]
pub struct CreatePlan {
    pub task: Task,
    pub tags: Vec<TaskTag>,
    pub files: Vec<wg_model::entity::TaskFile>,
    pub projects: Vec<TaskProject>,
    pub repositories: Vec<TaskRepository>,
    pub dependencies: Vec<TaskDependency>,
    pub event: TaskCreated,
    /// Whether the shell moves the new task from backlog to ready after the insert.
    pub ready: bool,
}

/// Plans the rows of a new task.
pub fn create_plan(
    stamp: &Stamp,
    allocation: Allocation<'_>,
    new: NewTask,
) -> Result<CreatePlan, WgError> {
    if let Some(key) = &new.key {
        check_tracker_key(
            &stamp.actor,
            key,
            allocation.key_prefix,
            allocation.key_taken,
        )?;
    }
    let key = match &new.key {
        Some(key) => TaskKey::parse(key)?,
        None => TaskKey::parse(&format!("{}-{}", allocation.key_prefix, allocation.number))?,
    };

    let actor = stamp.actor.to_string();
    let (task_id, ids) = stamp.ids().next();

    let tags = dedup(&new.tags);
    let project_ids = dedup(&new.projects);
    let sorted_projects: Vec<Id> = project_ids
        .iter()
        .copied()
        .collect::<BTreeSet<Id>>()
        .into_iter()
        .collect();

    let repositories: Vec<RepositoryLink> =
        new.repositories
            .iter()
            .fold(Vec::new(), |seen: Vec<RepositoryLink>, link| {
                if seen
                    .iter()
                    .any(|seen| seen.repository_id == link.repository_id)
                {
                    seen
                } else {
                    seen.into_iter().chain([link.clone()]).collect()
                }
            });

    let depends_on = dedup(&new.depends_on);
    let (dependencies, _) = depends_on.iter().fold(
        (Vec::with_capacity(depends_on.len()), ids),
        |(dependencies, ids): (Vec<TaskDependency>, _), depends_on_id| {
            let (id, ids) = ids.next();
            let row = TaskDependency {
                id,
                task_id,
                depends_on_id: *depends_on_id,
                created_by: actor.clone(),
                created_at: stamp.at,
            };
            (dependencies.into_iter().chain([row]).collect(), ids)
        },
    );

    let task = Task {
        id: task_id,
        workspace_id: allocation.workspace_id,
        number: allocation.number,
        key: key.clone(),
        description_hash: content_hash(&new.title, &new.description),
        title: new.title.clone(),
        description: new.description.clone(),
        acceptance_criteria: new.acceptance_criteria.iter().cloned().collect(),
        status: TaskStatus::Backlog,
        priority: new.priority,
        category: None,
        parent_id: new.parent_id,
        assignee: new.assignee.clone(),
        assigned_agent: new.assigned_agent.clone(),
        claimed_by: None,
        claim_expires_at: None,
        auto_execute: new.auto_execute,
        estimated_effort: new.estimated_effort.clone(),
        metadata: new.metadata.clone(),
        due_at: new.due_at,
        scheduled_at: new.scheduled_at,
        created_by: actor.clone(),
        version: 1,
        created_at: stamp.at,
        updated_at: stamp.at,
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    };

    let event = TaskCreated {
        key,
        title: new.title.clone(),
        description: new.description.clone(),
        acceptance_criteria: new.acceptance_criteria.iter().cloned().collect(),
        status: TaskStatus::Backlog,
        priority: new.priority,
        tags: tags.iter().cloned().collect::<VectorSync<String>>(),
        project_ids: sorted_projects.iter().copied().collect(),
        repository_ids: repositories.iter().map(|link| link.repository_id).collect(),
        parent_id: new.parent_id,
        depends_on: depends_on.iter().copied().collect(),
        assignee: new.assignee.clone(),
        auto_execute: new.auto_execute,
    };

    Ok(CreatePlan {
        tags: tags
            .iter()
            .map(|tag| TaskTag {
                task_id,
                tag: tag.clone(),
            })
            .collect(),
        files: file_rows(task_id, &new.files),
        projects: project_ids
            .iter()
            .map(|project_id| TaskProject {
                task_id,
                project_id: *project_id,
            })
            .collect(),
        repositories: repositories
            .into_iter()
            .map(|link| TaskRepository {
                task_id,
                repository_id: link.repository_id,
                access: link.access,
                created_at: stamp.at,
            })
            .collect(),
        dependencies,
        ready: new.status == TaskStatus::Ready,
        task,
        event,
    })
}

/// The values in first-seen order without repeats (`dict.fromkeys`).
fn dedup<T: Clone + PartialEq>(values: &[T]) -> Vec<T> {
    values
        .iter()
        .fold(Vec::with_capacity(values.len()), |out, value| {
            if out.contains(value) {
                out
            } else {
                out.into_iter().chain([value.clone()]).collect()
            }
        })
}
