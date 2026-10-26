//! The output shapes of the registry tools (`workspace_*`, `project_*`, `repository_*`). The wire
//! catalog (`wg_wire::dto::mcp_outputs`) keeps the shapes of the other tools; these are built from
//! the feature's DTOs, so they live here and the catalog check names them separately
//! (docs/rust-rewrite.md 4.16).

use wg_wire::dto::mcp_outputs::Shape;
use wg_wire::dto::repository_brief;
use wg_wire::dto::task_brief;

use super::dto::{
    project, project_summary, repository, repository_checkout, repository_remote,
    repository_status, workspace, workspace_summary,
};

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

const WORKSPACE: Shape = dto(&workspace::FIELDS);

const REPOSITORY: Shape = Shape::Dto {
    fields: &repository::FIELDS,
    nested: &[
        ("remotes", list(&dto(&repository_remote::FIELDS))),
        ("checkouts", list(&dto(&repository_checkout::FIELDS))),
    ],
};

/// The output shape of each registry tool, by tool name.
pub const OUTPUT_SHAPES: &[(&str, Shape)] = &[
    (
        "workspace_list",
        Shape::Object(&[("workspaces", list(&WORKSPACE))]),
    ),
    (
        "workspace_get",
        Shape::Dto {
            fields: &workspace_summary::FIELDS,
            nested: &[("workspace", WORKSPACE)],
        },
    ),
    (
        "project_list",
        Shape::Object(&[("projects", list(&dto(&project::FIELDS)))]),
    ),
    (
        "project_get",
        Shape::Dto {
            fields: &project_summary::FIELDS,
            nested: &[
                ("project", dto(&project::FIELDS)),
                ("repositories", list(&dto(&repository_brief::FIELDS))),
                ("recent_tasks", list(&dto(&task_brief::FIELDS))),
            ],
        },
    ),
    (
        "repository_list",
        Shape::Object(&[("repositories", list(&REPOSITORY))]),
    ),
    ("repository_get", REPOSITORY),
    ("repository_register", REPOSITORY),
    ("repository_status", dto(&repository_status::FIELDS)),
];
