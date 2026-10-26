//! `DecisionChoose`: the body that answers a decision.
//!
//! Request bodies export `NAME` and `FIELDS` only; see [`super::decision_option_in`].

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "DecisionChoose";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 3] = ["option_id", "other_text", "note"];
