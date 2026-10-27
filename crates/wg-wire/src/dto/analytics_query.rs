//! `AnalyticsQuery`: the body of `POST /analytics/query`, and its echo in a query result.

use wg_json::{Value, dict};

use crate::convention::{int, text, texts};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "AnalyticsQuery";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 5] = ["metric", "group_by", "period", "filters", "limit"];

/// The query as a result echoes it. `filters` is the [`super::analytics_filters::echo`] value.
pub fn echo(metric: &str, group_by: &[&str], period: &str, filters: Value, limit: i64) -> Value {
    dict!({
        "metric": text(metric),
        "group_by": texts(group_by.iter().copied()),
        "period": text(period),
        "filters": filters,
        "limit": int(limit),
    })
}
