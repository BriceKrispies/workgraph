//! `StackItemOut`.

use wg_model::entity::stack::StackItem;
use wg_model::entity::task::Task;
use wg_types::{Id, TaskKey, Timestamp};

use crate::convention::{display, model_z};
use crate::wire_dto;

wire_dto! {
    /// One child task of a stack, with the task's key and title joined in by the caller.
    pub struct StackItemOut<'a>("StackItemOut") value from_model(item: &'a StackItem, task: &'a Task) {
        id: Id = item.id,
        stack_id: Id = item.stack_id,
        task_id: Id = item.task_id,
        #[serde(serialize_with = "display")]
        task_key: &'a TaskKey = &task.key,
        task_title: &'a str = &task.title,
        state: &'a str = item.state.as_str(),
        merge_order: Option<i64> = item.merge_order,
        detail: Option<&'a str> = item.detail.as_deref(),
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = item.created_at,
        #[serde(serialize_with = "model_z")]
        updated_at: Timestamp = item.updated_at,
    }
}
