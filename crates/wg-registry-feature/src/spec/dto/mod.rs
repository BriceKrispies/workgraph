//! The registry feature's wire DTOs: the workspace, project and repository documents and the
//! settings documents. They moved out of `wg-wire` with the feature; the DTOs shared with other
//! features (`repository_brief`, `task_brief`, `stack_repository`) stay there.

pub mod project;
pub mod project_summary;
pub mod repository;
pub mod repository_checkout;
pub mod repository_remote;
pub mod repository_status;
pub mod settings;
pub mod workspace;
pub mod workspace_summary;

/// The DTOs of the registry feature: the OpenAPI component schema name and the field names in
/// order, as `wg_wire::dto::implemented` listed them before the move.
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
        (
            repository_status::NAME,
            repository_status::FIELDS.as_slice(),
        ),
    ]
}
