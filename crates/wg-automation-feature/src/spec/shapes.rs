//! The output shapes of the automation tools. The wire catalog (`wg_wire::dto::mcp_outputs`) keeps
//! the shapes of the other tools; the automation tools' shapes are the feature's, built from its
//! DTOs, so they live here and the catalog check names them separately (docs/rust-rewrite.md 4.16).

use wg_wire::dto::mcp_outputs::Shape;

use super::dto::automation;

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

/// The output shape of each automation tool, by tool name (M64-M65).
pub const OUTPUT_SHAPES: &[(&str, Shape)] = &[
    (
        "automation_list",
        Shape::Dto {
            fields: &automation::LIST_FIELDS,
            nested: &[
                ("rules", list(&dto(&automation::RULE_FIELDS))),
                ("recent_runs", list(&dto(&automation::RUN_FIELDS))),
            ],
        },
    ),
    ("automation_trigger", dto(&automation::RUN_FIELDS)),
];
