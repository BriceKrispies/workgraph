//! `DecisionOptionIn`: one option of a `DecisionRequest` body.
//!
//! Request bodies have no model values to convert, so they export `NAME` and `FIELDS` only; the
//! values are checked by `wg-validate` against the models of `contracts/`.

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "DecisionOptionIn";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 4] = ["id", "label", "detail", "recommended"];
