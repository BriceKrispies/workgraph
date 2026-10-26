//! The output shapes of the decision tools. The wire catalog (`wg_wire::dto::mcp_outputs`) keeps
//! the shapes of the other tools; the decision tools' shapes are the feature's, built from its
//! DTOs, so they live here and the catalog check names them separately (docs/rust-rewrite.md 4.16).

use wg_wire::dto::mcp_outputs::Shape;

use super::dto::{decision, decision_option, decision_task, decision_with_task};

/// A DTO whose nested fields are not looked into.
const fn dto(fields: &'static [&'static str]) -> Shape {
    Shape::Dto {
        fields,
        nested: &[],
    }
}

/// An array whose every element has the shape.
const fn list(item: &'static Shape) -> Shape {
    Shape::List(item)
}

const DECISION: Shape = Shape::Dto {
    fields: &decision::FIELDS,
    nested: &[("options", list(&dto(&decision_option::FIELDS)))],
};

const DECISION_WITH_TASK: Shape = Shape::Dto {
    fields: &decision_with_task::FIELDS,
    nested: &[
        ("options", list(&dto(&decision_option::FIELDS))),
        ("task", dto(&decision_task::FIELDS)),
    ],
};

/// The output shape of each decision tool, by tool name.
pub const OUTPUT_SHAPES: &[(&str, Shape)] = &[
    ("task_request_decision", DECISION),
    (
        "decision_list",
        Shape::Object(&[("decisions", list(&DECISION_WITH_TASK))]),
    ),
    (
        "task_decisions",
        Shape::Object(&[("decisions", list(&DECISION))]),
    ),
];
