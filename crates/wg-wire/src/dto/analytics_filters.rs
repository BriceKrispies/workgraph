//! `AnalyticsFilters`: the filter block of an analytics query. Its echo in a query result prints
//! the filters as `model_dump(mode="json")` does.

use wg_json::{Value, dict};
use wg_types::Timestamp;

use crate::convention::{optional_text, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "AnalyticsFilters";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 11] = [
    "project",
    "repository",
    "category",
    "agent",
    "provider",
    "model",
    "priority",
    "status",
    "kind",
    "since",
    "until",
];

/// The filter values of one query. An absent filter is `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Filters<'a> {
    pub project: Option<&'a str>,
    pub repository: Option<&'a str>,
    pub category: Option<&'a str>,
    pub agent: Option<&'a str>,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub priority: Option<&'a str>,
    pub status: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
}

/// The `Z` form of an optional bound. The echo is a dict whose values Python has already turned
/// into strings with `model_dump(mode="json")`, so the bound is text here, not a datetime value.
fn bound(at: Option<Timestamp>) -> Value {
    at.map_or(Value::Null, |at| text(&at.format_z()))
}

/// The echo of the filters inside a query result: the datetimes print in the model's `Z` form.
pub fn echo(filters: &Filters<'_>) -> Value {
    dict!({
        "project": optional_text(filters.project),
        "repository": optional_text(filters.repository),
        "category": optional_text(filters.category),
        "agent": optional_text(filters.agent),
        "provider": optional_text(filters.provider),
        "model": optional_text(filters.model),
        "priority": optional_text(filters.priority),
        "status": optional_text(filters.status),
        "kind": optional_text(filters.kind),
        "since": bound(filters.since),
        "until": bound(filters.until),
    })
}
