//! `DecisionWithdraw`: the body that withdraws an open decision.
//!
//! Request bodies export `NAME` and `FIELDS` only; see [`super::decision_option_in`].

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "DecisionWithdraw";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 1] = ["reason"];
