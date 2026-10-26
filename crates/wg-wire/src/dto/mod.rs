//! The DTOs, one module per area.
//!
//! Each DTO module exports `NAME` and `FIELDS` and a `from_model` constructor; see the crate
//! documentation for the conventions. Later wire tasks add their modules here and their DTOs to
//! [`implemented`].

pub mod mcp_shapes;
pub mod project;
pub mod project_summary;
pub mod repository;
pub mod repository_brief;
pub mod repository_checkout;
pub mod repository_remote;
pub mod repository_status;
pub mod workspace;
pub mod workspace_summary;

/// The DTOs that exist: the OpenAPI component schema name and the field names in order.
pub fn implemented() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        (workspace::NAME, workspace::FIELDS.as_slice()),
        (project::NAME, project::FIELDS.as_slice()),
        (
            workspace_summary::NAME,
            workspace_summary::FIELDS.as_slice(),
        ),
        (project_summary::NAME, project_summary::FIELDS.as_slice()),
        (repository::NAME, repository::FIELDS.as_slice()),
        (
            repository_remote::NAME,
            repository_remote::FIELDS.as_slice(),
        ),
        (
            repository_checkout::NAME,
            repository_checkout::FIELDS.as_slice(),
        ),
        (repository_brief::NAME, repository_brief::FIELDS.as_slice()),
        (
            repository_status::NAME,
            repository_status::FIELDS.as_slice(),
        ),
    ]
}
