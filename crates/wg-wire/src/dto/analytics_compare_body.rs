//! `CompareBody`: the body of `POST /analytics/compare`.

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "CompareBody";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 6] = [
    "metric", "a_since", "a_until", "b_since", "b_until", "group_by",
];
