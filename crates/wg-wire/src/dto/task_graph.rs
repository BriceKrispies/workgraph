//! `TaskGraph`.

use wg_json::Value;

use crate::convention::model;

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "TaskGraph";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 2] = ["nodes", "edges"];

/// The dependency graph: `GraphNode` and `GraphEdge` values, in the order the caller gives them.
pub fn from_model(nodes: Vec<Value>, edges: Vec<Value>) -> Value {
    model(
        NAME,
        vec![("nodes", Value::List(nodes)), ("edges", Value::List(edges))],
    )
}
