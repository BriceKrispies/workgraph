//! One disagreement of `GET /classification/disagreements`: a task whose succeeded
//! classifications name more than one category. Both shapes are dicts with no OpenAPI component
//! schema.

use wg_json::Value;
use wg_model::entity::classification::Classification;
use wg_types::Id;

use crate::wire_dto;

wire_dto! {
    /// One classification inside a disagreement.
    pub struct Item<'a>("ClassificationDisagreementItem") dict item_from_model(c: &'a Classification) {
        id: Id = c.id,
        category: Option<&'a str> = c.category.as_deref(),
        source: &'a str = &c.source,
        provider: Option<&'a str> = c.provider.as_deref(),
        model: Option<&'a str> = c.model.as_deref(),
        is_current: bool = c.is_current,
        confidence: Option<f64> = c.confidence.map(|c| c.0),
    }
}

wire_dto! {
    /// One disagreement: the task and its classifications built by [`item_from_model`].
    pub struct Disagreement<'a>("ClassificationDisagreementOut")
    dict from_parts(task_id: &'a str, task_key: &'a str, classifications: Vec<Value>) {
        task_id: &'a str = task_id,
        task_key: &'a str = task_key,
        classifications: Vec<Value> = classifications,
    }
}
