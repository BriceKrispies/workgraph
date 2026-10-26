//! `StackItemOut`.

use wg_json::Value;
use wg_model::entity::stack::StackItem;
use wg_model::entity::task::Task;

use crate::convention::{TimeFormat, id, model, text, timestamp};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "StackItemOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 10] = [
    "id",
    "stack_id",
    "task_id",
    "task_key",
    "task_title",
    "state",
    "merge_order",
    "detail",
    "created_at",
    "updated_at",
];

/// One child task of a stack, with the task's key and title joined in by the caller.
pub fn from_model(item: &StackItem, task: &Task) -> Value {
    model(
        NAME,
        vec![
            ("id", id(item.id)),
            ("stack_id", id(item.stack_id)),
            ("task_id", id(item.task_id)),
            ("task_key", text(&task.key.to_string())),
            ("task_title", text(&task.title)),
            ("state", text(item.state.as_str())),
            (
                "merge_order",
                item.merge_order
                    .map_or(Value::Null, |order| Value::Int(i128::from(order))),
            ),
            ("detail", item.detail.as_deref().map_or(Value::Null, text)),
            ("created_at", timestamp(item.created_at, TimeFormat::ModelZ)),
            ("updated_at", timestamp(item.updated_at, TimeFormat::ModelZ)),
        ],
    )
}
