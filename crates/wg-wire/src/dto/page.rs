//! `Page_TaskOut_`: the page of `TaskOut` values the task list returns.
//!
//! The Python class is the generic `Page[T]`; the OpenAPI component name is the instantiation.

use wg_json::Value;

use crate::wire_dto;

wire_dto! {
    /// One page of items. `next_offset` is `None` on the last page.
    pub struct Page("Page_TaskOut_")
    value from_model(items: Vec<Value>, total: i64, limit: i64, offset: i64, next_offset: Option<i64>) {
        items: Vec<Value> = items,
        total: i64 = total,
        limit: i64 = limit,
        offset: i64 = offset,
        next_offset: Option<i64> = next_offset,
    }
}
