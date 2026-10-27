//! `TaskGraph`.

use wg_json::Value;

use crate::wire_dto;

wire_dto! {
    /// The dependency graph: `GraphNode` and `GraphEdge` values, in the order the caller gives them.
    pub struct TaskGraph("TaskGraph") value from_model(nodes: Vec<Value>, edges: Vec<Value>) {
        nodes: Vec<Value> = nodes,
        edges: Vec<Value> = edges,
    }
}
