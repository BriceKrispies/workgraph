//! Shared by the subagent and agent-time reports: the workspace scope and time window of a report,
//! the verdict on each run (the `_runs` and `_load` loops of `analytics/subagents.py` and
//! `analytics/agent_time.py` select runs the same way), and the JSON readers that follow Python's
//! `_dict` and `_number` helpers.

use serde_json::{Map, Value, json};
use wg_types::Timestamp;
use wg_types::pyval::nonempty;

use crate::agg::{Groups, aggregate, lookup};
use crate::source::{AnalyticsInput, TaskRow};

/// `UNKNOWN_MODEL` and the `UNKNOWN` of both reports: a run with no model name.
pub const UNKNOWN: &str = "unknown";

/// The workspace a report reads, and the default workspace (`None` when there is none).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Scope {
    pub workspace_id: String,
    pub default_workspace_id: Option<String>,
}

impl Scope {
    /// `ws_id == default_id` in Python: false when there is no default workspace.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.default_workspace_id.as_deref() == Some(self.workspace_id.as_str())
    }

    /// Whether `task` belongs to the reported workspace.
    #[must_use]
    pub fn owns(&self, task: &TaskRow) -> bool {
        task.workspace_id.as_deref() == Some(self.workspace_id.as_str())
    }
}

/// The filters of a report: `since` inclusive, `until` exclusive, and an optional model name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Window {
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    pub model: Option<String>,
}

impl Window {
    /// Whether an instant lies inside the window (`at < since` and `at >= until` are excluded).
    #[must_use]
    pub fn contains(&self, at: Timestamp) -> bool {
        self.since.is_none_or(|since| at >= since) && self.until.is_none_or(|until| at < until)
    }

    /// Whether a run's model name passes the model filter.
    #[must_use]
    pub fn model_matches(&self, name: &str) -> bool {
        self.model.as_deref().is_none_or(|model| model == name)
    }
}

/// What a report does with one run. `unlinked` is set for a run with no task link, which the
/// Python code counts whether or not the run is kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict<T> {
    pub unlinked: bool,
    pub kind: Kind<T>,
}

/// The fate of a run that passed the window and model filters.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind<T> {
    /// An experiment trial (its telemetry carries `trial_ids`); reported by `experiment report`.
    Trial,
    /// Linked to a task in another workspace.
    Outside,
    /// Unlinked, and the reported workspace is not the default one.
    NotDefault,
    /// Timed reports only: the run has no time breakdown yet.
    NoBreakdown,
    /// The run is counted.
    Kept(T),
}

/// The tasks by id, first row wins, as an equality join finds them. A NULL id matches nothing.
#[must_use]
pub fn task_index(input: &AnalyticsInput) -> Groups<Option<String>, TaskRow> {
    aggregate(
        input.tasks.iter().cloned(),
        |task| task.id.clone(),
        |task| task.clone(),
        |first, _| first,
    )
}

/// `_dict(text)`: the JSON object a text column holds, or an empty object for anything else.
#[must_use]
pub fn json_object(text: Option<&str>) -> Map<String, Value> {
    match text.and_then(|text| serde_json::from_str::<Value>(text).ok()) {
        Some(Value::Object(object)) => object,
        _ => Map::new(),
    }
}

/// `_number(value)`: a JSON number as a float; booleans, strings and absent values are not numbers.
#[must_use]
pub fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        _ => None,
    }
}

/// `row.model or row.requested_model or UNKNOWN`.
#[must_use]
pub fn model_name<'a>(model: &'a Option<String>, requested: &'a Option<String>) -> &'a str {
    nonempty(model.as_deref())
        .or(nonempty(requested.as_deref()))
        .unwrap_or(UNKNOWN)
}

/// Python's `datetime.timestamp()` of a UTC instant: the exact microsecond count over a million.
#[must_use]
pub fn epoch_seconds(at: Timestamp) -> f64 {
    let micros = at.to_offset_datetime().unix_timestamp_nanos() / 1_000;
    micros as f64 / 1_000_000.0
}

/// `_epoch(text)` for a string timestamp; `None` when it does not parse.
#[must_use]
pub fn parse_epoch(text: Option<&str>) -> Option<f64> {
    Timestamp::parse_iso(text?).ok().map(epoch_seconds)
}

/// A JSON object from ordered pairs.
#[must_use]
pub fn object(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

/// An optional timestamp as Python's `isoformat()` text, or `null`.
#[must_use]
pub fn iso_value(at: Option<Timestamp>) -> Value {
    at.map_or(Value::Null, |at| Value::String(at.format_isoformat()))
}

/// `statistics.median` of floats as JSON: `null` over no values.
#[must_use]
pub fn median_json(values: impl IntoIterator<Item = f64>) -> Value {
    wg_pynum::median(values).map_or(Value::Null, Value::from)
}

/// The `window` object every report echoes.
#[must_use]
pub fn window_json(window: &Window) -> Value {
    json!({
        "since": iso_value(window.since),
        "until": iso_value(window.until),
    })
}

/// The task of a run's `task_id`, if the task exists.
#[must_use]
pub fn owner<'a>(
    tasks: &'a Groups<Option<String>, TaskRow>,
    task_id: &Option<String>,
) -> Option<&'a TaskRow> {
    lookup(tasks, task_id)
}
