//! `RetroBody`: the body of `POST /reports/retrospective`.

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "RetroBody";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 4] = ["period_days", "project", "repository", "use_model"];
