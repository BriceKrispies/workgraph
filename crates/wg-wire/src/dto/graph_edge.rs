//! `GraphEdge`.

use serde::Serialize;
use wg_types::Id;

use crate::wire_dto;

/// What an edge of the graph stands for. Its wire text is the `Literal` of the Python model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
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

wire_dto! {
    /// One edge: `source` to `target`. `relation_type` is set for relation edges only.
    pub struct GraphEdge<'a>("GraphEdge")
    value from_model(source: Id, target: Id, kind: EdgeKind, relation_type: Option<&'a str>) {
        source: Id = source,
        target: Id = target,
        kind: EdgeKind = kind,
        relation_type: Option<&'a str> = relation_type,
    }
}
