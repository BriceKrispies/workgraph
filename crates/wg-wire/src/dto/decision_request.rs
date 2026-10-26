//! `DecisionRequest`: the body that asks the user a question a task waits on.
//!
//! Request bodies export `NAME` and `FIELDS` only; see [`super::decision_option_in`].

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "DecisionRequest";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 2] = ["question", "options"];
