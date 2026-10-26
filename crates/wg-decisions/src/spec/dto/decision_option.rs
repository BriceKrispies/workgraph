//! `DecisionOptionOut`.

use wg_json::Value;
use wg_model::entity::task::DecisionOption;

use wg_wire::convention::{model, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "DecisionOptionOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 4] = ["id", "label", "detail", "recommended"];

/// One option a decision offers.
pub fn from_model(option: &DecisionOption) -> Value {
    model(
        NAME,
        vec![
            ("id", text(&option.id)),
            ("label", text(&option.label)),
            ("detail", text(&option.detail)),
            ("recommended", Value::Bool(option.recommended)),
        ],
    )
}
