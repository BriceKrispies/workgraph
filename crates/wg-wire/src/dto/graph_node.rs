//! `GraphNode`.

use wg_model::entity::task::{Task, TaskTag};
use wg_types::{Id, TaskKey, TaskPriority, TaskStatus};

use crate::convention::display;
use crate::wire_dto;

wire_dto! {
    /// One task of the dependency graph. `blocked` is the service's verdict (an unsatisfied
    /// dependency); the satisfaction rules are not decided here.
    pub struct GraphNode<'a>("GraphNode")
    value from_model(t: &'a Task, project_ids: &'a [Id], repository_ids: &'a [Id], tags: &'a [TaskTag], blocked: bool) {
        id: Id = t.id,
        #[serde(serialize_with = "display")]
        key: &'a TaskKey = &t.key,
        title: &'a str = &t.title,
        status: TaskStatus = t.status,
        priority: TaskPriority = t.priority,
        category: Option<&'a str> = t.category.as_deref(),
        parent_id: Option<Id> = t.parent_id,
        project_ids: &'a [Id] = project_ids,
        repository_ids: &'a [Id] = repository_ids,
        tags: Vec<&'a str> = tags.iter().map(|tag| tag.tag.as_str()).collect(),
        blocked: bool = blocked,
    }
}
