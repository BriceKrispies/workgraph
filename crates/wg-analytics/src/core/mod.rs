//! The pure core of the metric queries: the base relations, the metric registry, the period
//! buckets and the query engine. Nothing here reads a store, a clock or the environment; the
//! caller passes the [`AnalyticsInput`](crate::source::AnalyticsInput) and the [`Facts`](crate::Facts).

pub mod bases;
pub mod coupling;
pub mod failures;
pub mod metrics;
pub mod period;
pub mod query;
pub mod service;
pub mod stats;
