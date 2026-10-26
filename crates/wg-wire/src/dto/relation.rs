//! `RelationOut`.

use wg_json::Value;
use wg_model::entity::task::TaskRelation;

use crate::convention::{TimeFormat, id, model, text, timestamp};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "RelationOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 5] = [
    "id",
    "source_task_id",
    "target_task_id",
    "relation_type",
    "created_at",
];

/// A typed, non-blocking relation between two tasks.
pub fn from_model(relation: &TaskRelation) -> Value {
    model(
        NAME,
        vec![
            ("id", id(relation.id)),
            ("source_task_id", id(relation.source_task_id)),
            ("target_task_id", id(relation.target_task_id)),
            ("relation_type", text(relation.relation_type.as_str())),
            (
                "created_at",
                timestamp(relation.created_at, TimeFormat::ModelZ),
            ),
        ],
    )
}
