//! `StackCreate`: the request body of `POST /stacks`. It is parsed from the request, so it has no
//! `from_model` constructor.

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "StackCreate";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 1] = ["parent"];
