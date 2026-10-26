//! `ExportBody`: the body of `POST /analytics/export`.

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "ExportBody";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 3] = ["format", "dataset", "query"];
