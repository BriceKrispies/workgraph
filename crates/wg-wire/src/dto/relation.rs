//! `RelationOut`.

use wg_model::entity::task::TaskRelation;
use wg_types::{Id, RelationType, Timestamp};

use crate::convention::model_z;
use crate::wire_dto;

wire_dto! {
    /// A typed, non-blocking relation between two tasks.
    pub struct RelationOut("RelationOut") value from_model(r: &TaskRelation) {
        id: Id = r.id,
        source_task_id: Id = r.source_task_id,
        target_task_id: Id = r.target_task_id,
        relation_type: RelationType = r.relation_type,
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = r.created_at,
    }
}
