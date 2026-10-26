//! The agent-time reports: `GET /analytics/agent-time` (`breakdown`) and
//! `GET /analytics/bottlenecks` (`bottlenecks`), with their totals, class rows and group rows.
//!
//! Every datetime prints as `isoformat()` of an aware UTC datetime (the window bounds).

use wg_json::Value;
use wg_types::Timestamp;

use crate::convention::{TimeFormat, dict, optional_text, optional_timestamp, text, texts};

/// The name of the breakdown dict; it has no OpenAPI component schema.
pub const BREAKDOWN_NAME: &str = "AgentTimeOut";

/// The name of the bottleneck dict; it has no OpenAPI component schema.
pub const BOTTLENECKS_NAME: &str = "BottlenecksOut";

/// The fields of the breakdown, in order.
pub const BREAKDOWN_FIELDS: [&str; 6] = ["by", "window", "model", "totals", "notes", "rows"];

/// The fields of the bottleneck report, in order.
pub const BOTTLENECKS_FIELDS: [&str; 7] = [
    "window",
    "totals",
    "classes",
    "slowest_calls",
    "concurrency",
    "by_task",
    "notes",
];

/// The fields of `window`.
pub const WINDOW_FIELDS: [&str; 2] = ["since", "until"];

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

/// The `window` block: the optional bounds as `isoformat()` of aware UTC datetimes.
pub fn window(since: Option<Timestamp>, until: Option<Timestamp>) -> Value {
    dict(vec![
        ("since", optional_timestamp(since, TimeFormat::Isoformat)),
        ("until", optional_timestamp(until, TimeFormat::Isoformat)),
    ])
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

/// The breakdown. `rows` are class rows when `by` is `class`, group rows otherwise.
pub fn breakdown(
    by: &str,
    window: Value,
    model: Option<&str>,
    totals: Value,
    notes: &[&str],
    rows: Vec<Value>,
) -> Value {
    dict(vec![
        ("by", text(by)),
        ("window", window),
        ("model", optional_text(model)),
        ("totals", totals),
        ("notes", texts(notes.iter().copied())),
        ("rows", Value::List(rows)),
    ])
}

/// The bottleneck report. `classes` are class rows, `by_task` group rows.
pub fn bottlenecks(
    window: Value,
    totals: Value,
    classes: Vec<Value>,
    slowest_calls: Vec<Value>,
    concurrency: Value,
    by_task: Vec<Value>,
    notes: &[&str],
) -> Value {
    dict(vec![
        ("window", window),
        ("totals", totals),
        ("classes", Value::List(classes)),
        ("slowest_calls", Value::List(slowest_calls)),
        ("concurrency", concurrency),
        ("by_task", Value::List(by_task)),
        ("notes", texts(notes.iter().copied())),
    ])
}
