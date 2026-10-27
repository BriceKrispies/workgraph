//! `CommentOut`.

use serde_json::{Map, Value as Json};
use wg_model::entity::task::TaskComment;
use wg_types::{CommentKind, Id, Timestamp};

use crate::convention::{json_map, model_z};
use crate::wire_dto;

wire_dto! {
    /// A comment, progress report or result on a task.
    pub struct CommentOut<'a>("CommentOut") value from_model(c: &'a TaskComment) {
        id: Id = c.id,
        task_id: Id = c.task_id,
        attempt_id: Option<Id> = c.attempt_id,
        author: &'a str = &c.author,
        kind: CommentKind = c.kind,
        body: &'a str = &c.body,
        #[serde(serialize_with = "json_map")]
        data: &'a Map<String, Json> = &c.data,
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = c.created_at,
    }
}
