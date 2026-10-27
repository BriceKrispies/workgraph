//! The result of an analytics query (`POST /analytics/query`, MCP `analytics_query`).

use wg_json::{Value, dict};
use wg_types::Timestamp;

use crate::convention::{TimeFormat, int, text, texts, timestamp};

/// The name of the result dict; it has no OpenAPI component schema.
pub const NAME: &str = "AnalyticsQueryOut";

/// The fields in the order `AnalyticsService.query` builds them.
pub const FIELDS: [&str; 6] = [
    "metric",
    "query",
    "rows",
    "sample_size",
    "provenance",
    "notes",
];

/// The fields of `provenance`.
pub const PROVENANCE_FIELDS: [&str; 3] = ["sql", "source", "generated_at"];

/// The provenance block. `generated_at` is Python's `isoformat()` of an aware UTC datetime.
pub fn provenance(sql: &str, source: &str, generated_at: Timestamp) -> Value {
    dict!({
        "sql": text(sql),
        "source": text(source),
        "generated_at": timestamp(generated_at, TimeFormat::Isoformat),
    })
}

/// A query result. `rows` are the grouped rows the engine returned, each already in the
/// dimension-then-aggregate order of the SQL it ran.
pub fn from_parts(
    metric: Value,
    query: Value,
    rows: Vec<Value>,
    sample_size: i64,
    provenance: Value,
    notes: &[&str],
) -> Value {
    dict!({
        "metric": metric,
        "query": query,
        "rows": Value::List(rows),
        "sample_size": int(sample_size),
        "provenance": provenance,
        "notes": texts(notes.iter().copied()),
    })
}
