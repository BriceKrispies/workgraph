//! The agent-time reports: `GET /analytics/agent-time` (`breakdown`) and
//! `GET /analytics/bottlenecks` (`bottlenecks`), with their totals, class rows and group rows.
//!
//! Every datetime prints as `isoformat()` of an aware UTC datetime (the window bounds).

use wg_json::{Value, dict};
use wg_types::Timestamp;

use crate::convention::isoformat;
use crate::wire_dto;

/// The fields of `totals`, in order.
pub const TOTALS_FIELDS: [&str; 10] = [
    "runs",
    "span_seconds",
    "accounted_seconds",
    "tool_seconds",
    "model_seconds",
    "waiting_seconds",
    "polling_seconds",
    "model_share",
    "waiting_share",
    "polling_share",
];

/// The fields of one class row (`_class_rows`), in order.
pub const CLASS_FIELDS: [&str; 5] = ["class", "kind", "seconds", "calls", "share"];

/// The fields of one group row (`_group_rows`), in order.
pub const GROUP_FIELDS: [&str; 9] = [
    "key",
    "runs",
    "span_seconds",
    "accounted_seconds",
    "model_seconds",
    "waiting_seconds",
    "polling_share",
    "top_classes",
    "classes",
];

/// The fields of one slowest call, in order.
pub const SLOWEST_FIELDS: [&str; 9] = [
    "class",
    "seconds",
    "tool",
    "command",
    "at",
    "description",
    "model",
    "task",
    "concurrent_runs",
];

/// The fields of `concurrency`, in order.
pub const CONCURRENCY_FIELDS: [&str; 3] =
    ["peak_during_window", "median_at_slowest_calls", "scope"];

wire_dto! {
    /// The `window` block: the optional bounds as `isoformat()` of aware UTC datetimes.
    pub struct Window("Window") dict window(since: Option<Timestamp>, until: Option<Timestamp>) {
        #[serde(serialize_with = "isoformat")]
        since: Option<Timestamp> = since,
        #[serde(serialize_with = "isoformat")]
        until: Option<Timestamp> = until,
    }
}

/// The `totals` block; the values are in [`TOTALS_FIELDS`] order.
pub fn totals(values: [Value; 10]) -> Value {
    dict(TOTALS_FIELDS.into_iter().zip(values).collect())
}

/// One class row; the values are in [`CLASS_FIELDS`] order.
pub fn class_row(values: [Value; 5]) -> Value {
    dict(CLASS_FIELDS.into_iter().zip(values).collect())
}

/// One group row; the values are in [`GROUP_FIELDS`] order.
pub fn group_row(values: [Value; 9]) -> Value {
    dict(GROUP_FIELDS.into_iter().zip(values).collect())
}

/// One slowest call; the values are in [`SLOWEST_FIELDS`] order.
pub fn slowest_call(values: [Value; 9]) -> Value {
    dict(SLOWEST_FIELDS.into_iter().zip(values).collect())
}

/// The `concurrency` block; the values are in [`CONCURRENCY_FIELDS`] order.
pub fn concurrency(values: [Value; 3]) -> Value {
    dict(CONCURRENCY_FIELDS.into_iter().zip(values).collect())
}

wire_dto! {
    /// The breakdown. `rows` are class rows when `by` is `class`, group rows otherwise.
    pub struct Breakdown<'a>("AgentTimeOut")
    dict breakdown(by: &'a str, window: Value, model: Option<&'a str>, totals: Value, notes: &'a [&'a str], rows: Vec<Value>) {
        by: &'a str = by,
        window: Value = window,
        model: Option<&'a str> = model,
        totals: Value = totals,
        notes: &'a [&'a str] = notes,
        rows: Vec<Value> = rows,
    }
}

wire_dto! {
    /// The bottleneck report. `classes` are class rows, `by_task` group rows.
    pub struct Bottlenecks<'a>("BottlenecksOut")
    dict bottlenecks(
        window: Value,
        totals: Value,
        classes: Vec<Value>,
        slowest_calls: Vec<Value>,
        concurrency: Value,
        by_task: Vec<Value>,
        notes: &'a [&'a str]
    ) {
        window: Value = window,
        totals: Value = totals,
        classes: Vec<Value> = classes,
        slowest_calls: Vec<Value> = slowest_calls,
        concurrency: Value = concurrency,
        by_task: Vec<Value> = by_task,
        notes: &'a [&'a str] = notes,
    }
}
