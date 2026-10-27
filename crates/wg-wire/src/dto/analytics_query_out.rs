//! The result of an analytics query (`POST /analytics/query`, MCP `analytics_query`).

use wg_json::Value;
use wg_types::Timestamp;

use crate::convention::isoformat;
use crate::wire_dto;

wire_dto! {
    /// The provenance block. `generated_at` is Python's `isoformat()` of an aware UTC datetime.
    pub struct Provenance<'a>("Provenance")
    dict provenance(sql: &'a str, source: &'a str, generated_at: Timestamp) {
        sql: &'a str = sql,
        source: &'a str = source,
        #[serde(serialize_with = "isoformat")]
        generated_at: Timestamp = generated_at,
    }
}

wire_dto! {
    /// A query result. `rows` are the grouped rows the engine returned, each already in the
    /// dimension-then-aggregate order of the SQL it ran.
    pub struct AnalyticsQueryOut<'a>("AnalyticsQueryOut")
    dict from_parts(metric: Value, query: Value, rows: Vec<Value>, sample_size: i64, provenance: Value, notes: &'a [&'a str]) {
        metric: Value = metric,
        query: Value = query,
        rows: Vec<Value> = rows,
        sample_size: i64 = sample_size,
        provenance: Value = provenance,
        notes: &'a [&'a str] = notes,
    }
}
