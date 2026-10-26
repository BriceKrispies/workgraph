//! [`TaskSlice`]: tasks and their child rows, with the registry rows they refer to.

use std::collections::BTreeMap;

use rpds::RedBlackTreeMapSync;
use wg_types::{Id, TaskKey};

use crate::entity::{
    ExternalItem, Task, TaskComment, TaskDecision, TaskDependency, TaskFile, TaskLink, TaskProject,
    TaskRelation, TaskRepository, TaskTag,
};
use crate::slice::{RegistrySlice, children};
use crate::write::{Row, RowKey, Write};

/// Loaded rows of the task family, plus the workspaces, projects and repositories they name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskSlice {
    pub registry: RegistrySlice,
    pub tasks: RedBlackTreeMapSync<Id, Task>,
    /// Keyed `(task_id, position)`.
    pub files: RedBlackTreeMapSync<(Id, i64), TaskFile>,
    /// Keyed `(task_id, tag)`.
    pub tags: RedBlackTreeMapSync<(Id, String), TaskTag>,
    pub links: RedBlackTreeMapSync<Id, TaskLink>,
    pub comments: RedBlackTreeMapSync<Id, TaskComment>,
    pub dependencies: RedBlackTreeMapSync<Id, TaskDependency>,
    pub relations: RedBlackTreeMapSync<Id, TaskRelation>,
    /// Keyed `(task_id, project_id)`.
    pub task_projects: RedBlackTreeMapSync<(Id, Id), TaskProject>,
    /// Keyed `(task_id, repository_id)`.
    pub task_repositories: RedBlackTreeMapSync<(Id, Id), TaskRepository>,
    pub external_items: RedBlackTreeMapSync<Id, ExternalItem>,
    /// Keyed by decision id. Decisions of a task are the rows whose `task_id` is that task.
    pub decisions: RedBlackTreeMapSync<Id, TaskDecision>,
}

impl TaskSlice {
    pub fn task(&self, id: Id) -> Option<&Task> {
        self.tasks.get(&id)
    }

    /// The task with this key in this workspace.
    pub fn task_by_key(&self, workspace_id: Id, key: &TaskKey) -> Option<&Task> {
        self.tasks
            .values()
            .find(|task| task.workspace_id == workspace_id && task.key == *key)
    }

    /// The files of a task, by position.
    pub fn files_of(&self, task_id: Id) -> impl Iterator<Item = &TaskFile> {
        children(&self.files, task_id)
    }

    /// The tags of a task, sorted (the order of `Task.tags` in Python).
    pub fn tags_of(&self, task_id: Id) -> impl Iterator<Item = &str> {
        children(&self.tags, task_id).map(|row| row.tag.as_str())
    }

    /// The links of a task, by link id.
    pub fn links_of(&self, task_id: Id) -> impl Iterator<Item = &TaskLink> {
        self.links
            .values()
            .filter(move |row| row.task_id == task_id)
    }

    /// The comments of a task, by comment id (sort by `created_at` for display order).
    pub fn comments_of(&self, task_id: Id) -> impl Iterator<Item = &TaskComment> {
        self.comments
            .values()
            .filter(move |row| row.task_id == task_id)
    }

    /// The dependencies of a task (what it waits for), by dependency id.
    pub fn dependencies_of(&self, task_id: Id) -> impl Iterator<Item = &TaskDependency> {
        self.dependencies
            .values()
            .filter(move |row| row.task_id == task_id)
    }

    /// The dependencies on a task (what waits for it), by dependency id.
    pub fn dependents_of(&self, task_id: Id) -> impl Iterator<Item = &TaskDependency> {
        self.dependencies
            .values()
            .filter(move |row| row.depends_on_id == task_id)
    }

    /// The relations from a task, by relation id.
    pub fn relations_of(&self, task_id: Id) -> impl Iterator<Item = &TaskRelation> {
        self.relations
            .values()
            .filter(move |row| row.source_task_id == task_id)
    }

    /// The project memberships of a task, by project id.
    pub fn projects_of(&self, task_id: Id) -> impl Iterator<Item = &TaskProject> {
        children(&self.task_projects, task_id)
    }

    /// The repository memberships of a task, by repository id.
    pub fn repositories_of(&self, task_id: Id) -> impl Iterator<Item = &TaskRepository> {
        children(&self.task_repositories, task_id)
    }

    /// The decisions of a task, oldest first (`created_at`, then id).
    pub fn decisions_of(&self, task_id: Id) -> Vec<&TaskDecision> {
        self.decisions
            .values()
            .filter(|row| row.task_id == task_id)
            .map(|row| ((row.created_at, row.id), row))
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect()
    }

    /// The tracker item a task mirrors, if any.
    pub fn external_item_of(&self, task_id: Id) -> Option<&ExternalItem> {
        self.external_items
            .values()
            .find(|row| row.task_id == task_id)
    }

    /// The slice after `write`. Registry rows go to [`TaskSlice::registry`]; writes to tables this
    /// slice does not hold leave it unchanged.
    #[must_use]
    pub fn apply(&self, write: &Write) -> Self {
        match write {
            Write::Insert(row) | Write::Update { row, .. } => self.put(row, write),
            Write::Delete(key) => self.remove(key, write),
        }
    }

    /// The slice after every write, in order.
    #[must_use]
    pub fn apply_all<'a>(&self, writes: impl IntoIterator<Item = &'a Write>) -> Self {
        writes
            .into_iter()
            .fold(self.clone(), |slice, write| slice.apply(write))
    }

    fn put(&self, row: &Row, write: &Write) -> Self {
        match row {
            Row::Task(row) => TaskSlice {
                tasks: self.tasks.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::TaskFile(row) => TaskSlice {
                files: self.files.insert((row.task_id, row.position), row.clone()),
                ..self.clone()
            },
            Row::TaskTag(row) => TaskSlice {
                tags: self
                    .tags
                    .insert((row.task_id, row.tag.clone()), row.clone()),
                ..self.clone()
            },
            Row::TaskLink(row) => TaskSlice {
                links: self.links.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::TaskComment(row) => TaskSlice {
                comments: self.comments.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::TaskDependency(row) => TaskSlice {
                dependencies: self.dependencies.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::TaskRelation(row) => TaskSlice {
                relations: self.relations.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::TaskProject(row) => TaskSlice {
                task_projects: self
                    .task_projects
                    .insert((row.task_id, row.project_id), row.clone()),
                ..self.clone()
            },
            Row::TaskRepository(row) => TaskSlice {
                task_repositories: self
                    .task_repositories
                    .insert((row.task_id, row.repository_id), row.clone()),
                ..self.clone()
            },
            Row::ExternalItem(row) => TaskSlice {
                external_items: self.external_items.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::TaskDecision(row) => TaskSlice {
                decisions: self.decisions.insert(row.id, row.clone()),
                ..self.clone()
            },
            _ => TaskSlice {
                registry: self.registry.apply(write),
                ..self.clone()
            },
        }
    }

    fn remove(&self, key: &RowKey, write: &Write) -> Self {
        match key {
            RowKey::Task(id) => TaskSlice {
                tasks: self.tasks.remove(id),
                ..self.clone()
            },
            RowKey::TaskFile { task_id, position } => TaskSlice {
                files: self.files.remove(&(*task_id, *position)),
                ..self.clone()
            },
            RowKey::TaskTag { task_id, tag } => TaskSlice {
                tags: self.tags.remove(&(*task_id, tag.clone())),
                ..self.clone()
            },
            RowKey::TaskLink(id) => TaskSlice {
                links: self.links.remove(id),
                ..self.clone()
            },
            RowKey::TaskComment(id) => TaskSlice {
                comments: self.comments.remove(id),
                ..self.clone()
            },
            RowKey::TaskDependency(id) => TaskSlice {
                dependencies: self.dependencies.remove(id),
                ..self.clone()
            },
            RowKey::TaskRelation(id) => TaskSlice {
                relations: self.relations.remove(id),
                ..self.clone()
            },
            RowKey::TaskProject {
                task_id,
                project_id,
            } => TaskSlice {
                task_projects: self.task_projects.remove(&(*task_id, *project_id)),
                ..self.clone()
            },
            RowKey::TaskRepository {
                task_id,
                repository_id,
            } => TaskSlice {
                task_repositories: self.task_repositories.remove(&(*task_id, *repository_id)),
                ..self.clone()
            },
            RowKey::ExternalItem(id) => TaskSlice {
                external_items: self.external_items.remove(id),
                ..self.clone()
            },
            RowKey::TaskDecision(id) => TaskSlice {
                decisions: self.decisions.remove(id),
                ..self.clone()
            },
            _ => TaskSlice {
                registry: self.registry.apply(write),
                ..self.clone()
            },
        }
    }
}
