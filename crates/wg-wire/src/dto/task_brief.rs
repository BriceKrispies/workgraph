//! `TaskBrief`.

use wg_model::entity::task::Task;
use wg_types::{Id, TaskKey, TaskPriority, TaskStatus};

use crate::convention::display;
use crate::wire_dto;

wire_dto! {
    /// The short form of a task, as a dependency or a graph neighbour is shown.
    pub struct TaskBrief<'a>("TaskBrief") value from_model(t: &'a Task) {
        id: Id = t.id,
        #[serde(serialize_with = "display")]
        key: &'a TaskKey = &t.key,
        title: &'a str = &t.title,
        status: TaskStatus = t.status,
        priority: TaskPriority = t.priority,
        category: Option<&'a str> = t.category.as_deref(),
    }
}
