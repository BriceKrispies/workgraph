//! A metric definition: one entry of the metric catalog (`GET /analytics/metrics`, `catalog`
//! reports and the metric block of every query result).

use wg_json::Value;

use crate::convention::{dict, text, texts};

/// The name of the definition dict; it has no OpenAPI component schema.
pub const NAME: &str = "MetricDefinition";

/// The fields in the order `metric_definition` builds them.
pub const FIELDS: [&str; 9] = [
    "id",
    "family",
    "title",
    "definition",
    "unit",
    "statistic",
    "time_column",
    "dimensions",
    "caveats",
];

/// The values of one metric. `dimensions` must already be sorted, as the Python builds them.
#[derive(Debug, Clone)]
pub struct MetricDefinition<'a> {
    pub id: &'a str,
    pub family: &'a str,
    pub title: &'a str,
    pub definition: &'a str,
    pub unit: &'a str,
    pub statistic: &'a str,
    pub time_column: &'a str,
    pub dimensions: Vec<&'a str>,
    pub caveats: Vec<&'a str>,
}

/// One metric definition as the API prints it.
pub fn from_parts(metric: &MetricDefinition<'_>) -> Value {
    dict(vec![
        ("id", text(metric.id)),
        ("family", text(metric.family)),
        ("title", text(metric.title)),
        ("definition", text(metric.definition)),
        ("unit", text(metric.unit)),
        ("statistic", text(metric.statistic)),
        ("time_column", text(metric.time_column)),
        ("dimensions", texts(metric.dimensions.iter().copied())),
        ("caveats", texts(metric.caveats.iter().copied())),
    ])
}
