//! A metric definition: one entry of the metric catalog (`GET /analytics/metrics`, `catalog`
//! reports and the metric block of every query result).

use wg_json::Value;

use crate::dto::Dto;
use crate::wire_dto;

wire_dto! {
    /// The values of one metric, fields in the order `metric_definition` builds them.
    /// `dimensions` must already be sorted, as the Python builds them.
    #[derive(Clone)]
    pub struct MetricDefinition<'a>("MetricDefinition") {
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
}

/// One metric definition as the API prints it.
pub fn from_parts(metric: &MetricDefinition<'_>) -> Value {
    metric.dict()
}
