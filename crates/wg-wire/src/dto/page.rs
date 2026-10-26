//! `Page_TaskOut_`: the page of `TaskOut` values the task list returns.
//!
//! The Python class is the generic `Page[T]`; the OpenAPI component name is the instantiation.

use wg_json::Value;

use crate::convention::model;

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "Page_TaskOut_";

/// The fields in Python declaration order (`Page[T]`).
pub const FIELDS: [&str; 5] = ["items", "total", "limit", "offset", "next_offset"];

/// One page of items. `next_offset` is `None` on the last page.
pub fn from_model(
    items: Vec<Value>,
    total: i64,
    limit: i64,
    offset: i64,
    next_offset: Option<i64>,
) -> Value {
    model(
        NAME,
        vec![
            ("items", Value::List(items)),
            ("total", Value::Int(i128::from(total))),
            ("limit", Value::Int(i128::from(limit))),
            ("offset", Value::Int(i128::from(offset))),
            (
                "next_offset",
                next_offset.map_or(Value::Null, |next| Value::Int(i128::from(next))),
            ),
        ],
    )
}
