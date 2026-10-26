//! `CommentOut`.

use wg_json::Value;
use wg_model::entity::task::TaskComment;

use crate::convention::{TimeFormat, id, json_object, model, optional_id, text, timestamp};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "CommentOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 8] = [
    "id",
    "task_id",
    "attempt_id",
    "author",
    "kind",
    "body",
    "data",
    "created_at",
];

/// A comment, progress report or result on a task.
pub fn from_model(comment: &TaskComment) -> Value {
    model(
        NAME,
        vec![
            ("id", id(comment.id)),
            ("task_id", id(comment.task_id)),
            ("attempt_id", optional_id(comment.attempt_id)),
            ("author", text(&comment.author)),
            ("kind", text(comment.kind.as_str())),
            ("body", text(&comment.body)),
            ("data", json_object(&comment.data)),
            (
                "created_at",
                timestamp(comment.created_at, TimeFormat::ModelZ),
            ),
        ],
    )
}
