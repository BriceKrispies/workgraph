//! The task graph view (GV-19). Mirrors `TaskService.graph` in `core/services/tasks.py`: nodes
//! for the live tasks of a workspace, edges for dependencies, parents and typed relations.
//!
//! Nodes are in task-number order. Edges are in database order: the dependency rows by (task,
//! depended-on task), then the parent edges by child number, then the relation rows by (source,
//! target, type), each as SQLite reads the table through its index (`TaskService.graph`). A
//! dependency edge runs from the task depended on to the dependent.

use std::collections::{BTreeMap, BTreeSet};

use wg_model::slice::StackSlice;
use wg_model::{Task, TaskRelation, TaskSlice};
use wg_types::{Id, RelationType, TaskPriority, TaskStatus};

use crate::deps::satisfied_dependencies;
use crate::hierarchy::descendants;

/// Which tasks the graph shows.
#[derive(Debug, Clone, Copy)]
pub struct GraphQuery {
    pub workspace_id: Id,
    /// Only tasks in this project.
    pub project: Option<Id>,
    /// Only tasks linked to this repository.
    pub repository: Option<Id>,
    /// Only this task, its descendants and their direct dependency neighbours.
    pub root: Option<Id>,
    /// Keep accepted and cancelled tasks.
    pub include_closed: bool,
}

/// The kind of a [`GraphEdge`], by wire string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GraphEdgeKind {
    DependsOn,
    Parent,
    Relation,
}

impl GraphEdgeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DependsOn => "depends_on",
            Self::Parent => "parent",
            Self::Relation => "relation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    pub id: Id,
    pub key: String,
    pub title: String,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub category: Option<String>,
    /// The parent, when the parent is in the graph too.
    pub parent_id: Option<Id>,
    pub project_ids: Vec<Id>,
    pub repository_ids: Vec<Id>,
    pub tags: Vec<String>,
    /// Blocked by status, or waiting on an unsatisfied dependency shown in the graph.
    pub blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    pub source: Id,
    pub target: Id,
    pub kind: GraphEdgeKind,
    pub relation_type: Option<RelationType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// The tasks the query keeps, by id.
fn selected<'a>(tasks: &'a TaskSlice, query: &GraphQuery) -> BTreeMap<Id, &'a Task> {
    let live: BTreeMap<Id, &Task> = tasks
        .tasks
        .values()
        .filter(|task| {
            task.workspace_id == query.workspace_id
                && task.archived_at.is_none()
                && (query.include_closed
                    || !matches!(task.status, TaskStatus::Accepted | TaskStatus::Cancelled))
        })
        .map(|task| (task.id, task))
        .collect();
    let in_scope: BTreeMap<Id, &Task> = match query.root {
        None => live,
        Some(root) => {
            let below: BTreeSet<Id> = std::iter::once(root)
                .chain(descendants(tasks, root).into_iter().map(|task| task.id))
                .collect();
            let neighbours: BTreeSet<Id> = tasks
                .dependencies
                .values()
                .filter(|row| below.contains(&row.task_id) || below.contains(&row.depends_on_id))
                .flat_map(|row| [row.task_id, row.depends_on_id])
                .collect();
            live.into_iter()
                .filter(|(id, _)| below.contains(id) || neighbours.contains(id))
                .collect()
        }
    };
    in_scope
        .into_iter()
        .filter(|(id, _)| {
            query
                .project
                .is_none_or(|project| tasks.task_projects.contains_key(&(*id, project)))
                && query.repository.is_none_or(|repository| {
                    tasks.task_repositories.contains_key(&(*id, repository))
                })
        })
        .collect()
}

/// The graph of the tasks the query selects.
pub fn task_graph(tasks: &TaskSlice, stacks: &StackSlice, query: &GraphQuery) -> TaskGraph {
    let shown = selected(tasks, query);
    // Database order: `task_dependencies` is read through its (task_id, depends_on_id) index.
    let dep_rows: Vec<(Id, Id)> = tasks
        .dependencies
        .values()
        .filter(|row| shown.contains_key(&row.task_id) && shown.contains_key(&row.depends_on_id))
        .map(|row| (row.task_id, row.depends_on_id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // The tasks in number order, as the task query yields them (the index on (workspace, number)).
    let by_number: Vec<&Task> = shown
        .values()
        .map(|task| (task.number, *task))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    let satisfied = satisfied_dependencies(tasks, stacks, &dep_rows);
    let unsatisfied: BTreeSet<Id> = dep_rows
        .iter()
        .filter(|edge| !satisfied.contains(edge))
        .map(|(task_id, _)| *task_id)
        .collect();
    let nodes: Vec<GraphNode> = by_number
        .iter()
        .map(|task| GraphNode {
            id: task.id,
            key: task.key.to_string(),
            title: task.title.clone(),
            status: task.status,
            priority: task.priority,
            category: task.category.clone(),
            parent_id: task.parent_id.filter(|parent| shown.contains_key(parent)),
            project_ids: tasks
                .projects_of(task.id)
                .map(|row| row.project_id)
                .collect(),
            repository_ids: tasks
                .repositories_of(task.id)
                .map(|row| row.repository_id)
                .collect(),
            tags: tasks
                .tags_of(task.id)
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            blocked: task.status == TaskStatus::Blocked || unsatisfied.contains(&task.id),
        })
        .collect();
    let dependency_edges = dep_rows.iter().map(|(task_id, dep_id)| GraphEdge {
        source: *dep_id,
        target: *task_id,
        kind: GraphEdgeKind::DependsOn,
        relation_type: None,
    });
    let parent_edges = by_number.iter().filter_map(|task| {
        task.parent_id
            .filter(|parent| shown.contains_key(parent))
            .map(|parent| GraphEdge {
                source: parent,
                target: task.id,
                kind: GraphEdgeKind::Parent,
                relation_type: None,
            })
    });
    // Relations in the order `task_relations` is read through its (source, target, type) index.
    let relation_rows: BTreeMap<(Id, Id, &str), &TaskRelation> = tasks
        .relations
        .values()
        .filter(|rel| {
            shown.contains_key(&rel.source_task_id) && shown.contains_key(&rel.target_task_id)
        })
        .map(|rel| {
            (
                (
                    rel.source_task_id,
                    rel.target_task_id,
                    rel.relation_type.as_str(),
                ),
                rel,
            )
        })
        .collect();
    let relation_edges = relation_rows.into_values().map(|rel| GraphEdge {
        source: rel.source_task_id,
        target: rel.target_task_id,
        kind: GraphEdgeKind::Relation,
        relation_type: Some(rel.relation_type),
    });
    TaskGraph {
        nodes,
        edges: dependency_edges
            .chain(parent_edges)
            .chain(relation_edges)
            .collect(),
    }
}
