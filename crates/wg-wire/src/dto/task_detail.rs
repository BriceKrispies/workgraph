//! `TaskDetail`: `TaskOut` plus the relations, comments, rollup and dependency state.

use wg_json::Value;
use wg_model::entity::task::{Task, TaskComment, TaskRelation};

use crate::dto::Dto;
use crate::dto::comment::CommentOut;
use crate::dto::relation::RelationOut;
use crate::dto::task::{TaskOut, TaskRelated};
use crate::wire_dto;

wire_dto! {
    /// The task with its relations and comments. `rollup` and `dependency_state` are the values the
    /// caller computed (see `TaskRollup` and `DependencyState`), or `None` when the task has none.
    pub struct TaskDetail<'a>("TaskDetail") extends TaskOut<'a>, fields_of TaskOut<'static> {
        pub relations: Vec<RelationOut>,
        pub comments: Vec<CommentOut<'a>>,
        pub rollup: Option<Value>,
        pub dependency_state: Option<Value>,
    }
}

/// The task with its relations and comments.
pub fn from_model(
    task: &Task,
    related: &TaskRelated<'_>,
    relations: &[TaskRelation],
    comments: &[TaskComment],
    rollup: Option<Value>,
    dependency_state: Option<Value>,
) -> Value {
    TaskDetail {
        base: TaskOut::new(task, related),
        relations: relations.iter().map(RelationOut::new).collect(),
        comments: comments.iter().map(CommentOut::new).collect(),
        rollup,
        dependency_state,
    }
    .value()
}
