//! `AnalyticsQuery`: the body of `POST /analytics/query`, and its echo in a query result.

use wg_json::Value;

use crate::wire_dto;

wire_dto! {
    /// The query as a result echoes it. `filters` is the [`super::analytics_filters::echo`] value.
    pub struct QueryEcho<'a>("AnalyticsQuery")
    dict echo(metric: &'a str, group_by: &'a [&'a str], period: &'a str, filters: Value, limit: i64) {
        metric: &'a str = metric,
        group_by: &'a [&'a str] = group_by,
        period: &'a str = period,
        filters: Value = filters,
        limit: i64 = limit,
    }
}
