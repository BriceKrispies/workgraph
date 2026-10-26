//! `GraphEdge`.

use wg_json::Value;
use wg_types::Id;

use crate::convention::{id, model, optional_text, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "GraphEdge";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 4] = ["source", "target", "kind", "relation_type"];

/// What an edge of the graph stands for. Its wire text is the `Literal` of the Python model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    DependsOn,
    Parent,
    Relation,
}

impl EdgeKind {
    /// The wire text.
    pub const fn as_str(self) -> &'static str {
        match self {
            EdgeKind::DependsOn => "depends_on",
            EdgeKind::Parent => "parent",
            EdgeKind::Relation => "relation",
        }
    }
}

/// One edge: `source` to `target`. `relation_type` is set for relation edges only.
pub fn from_model(source: Id, target: Id, kind: EdgeKind, relation_type: Option<&str>) -> Value {
    model(
        NAME,
        vec![
            ("source", id(source)),
            ("target", id(target)),
            ("kind", text(kind.as_str())),
            ("relation_type", optional_text(relation_type)),
        ],
    )
}
