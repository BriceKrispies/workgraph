//! `ClassificationOut`: one classification of a task, as `classification_get` returns it inside
//! `TaskClassifications`. It is not a component of the OpenAPI document, so the name is the
//! Python class name only.

use rpds::VectorSync;
use wg_model::entity::classification::Classification;
use wg_types::{Id, Timestamp};

use crate::convention::model_z;
use crate::wire_dto;

wire_dto! {
    /// A classification row. A Pydantic model, so `created_at` prints in the `Z` form.
    pub struct ClassificationOut<'a>("ClassificationOut") value from_model(c: &'a Classification) {
        id: Id = c.id,
        task_id: Id = c.task_id,
        source: &'a str = &c.source,
        status: &'a str = &c.status,
        provider: Option<&'a str> = c.provider.as_deref(),
        model: Option<&'a str> = c.model.as_deref(),
        prompt_version: Option<&'a str> = c.prompt_version.as_deref(),
        taxonomy_version: &'a str = &c.taxonomy_version,
        input_hash: &'a str = &c.input_hash,
        category: Option<&'a str> = c.category.as_deref(),
        subtype: Option<&'a str> = c.subtype.as_deref(),
        domain: Option<&'a str> = c.domain.as_deref(),
        affected_systems: &'a VectorSync<String> = &c.affected_systems,
        complexity: Option<&'a str> = c.complexity.as_deref(),
        tags: &'a VectorSync<String> = &c.tags,
        confidence: Option<f64> = c.confidence.map(|c| c.0),
        rationale: Option<&'a str> = c.rationale.as_deref(),
        validation_status: &'a str = &c.validation_status,
        error: Option<&'a str> = c.error.as_deref(),
        duration_ms: Option<i64> = c.duration_ms,
        is_current: bool = c.is_current,
        confirmed_by: Option<&'a str> = c.confirmed_by.as_deref(),
        batch_id: Option<Id> = c.batch_id,
        created_by: &'a str = &c.created_by,
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = c.created_at,
    }
}
