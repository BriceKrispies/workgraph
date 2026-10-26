//! `StackItemAdd`: the request body of `POST /stacks/{ref}/items`. It is parsed from the request,
//! so it has no `from_model` constructor.

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "StackItemAdd";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 1] = ["task"];
