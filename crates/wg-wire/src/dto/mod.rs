//! The DTOs, one module per area.
//!
//! Each DTO module exports `NAME` and `FIELDS` and a `from_model` constructor; see the crate
//! documentation for the conventions. Later wire tasks add their modules here and their DTOs to
//! [`implemented`].

pub mod agents;
pub mod automation;
pub mod claude;
pub mod event;
pub mod health;
pub mod llm;
pub mod notification;
pub mod project;
pub mod settings;
pub mod workspace;

/// The DTOs that exist: the OpenAPI component schema name and the field names in order.
pub fn implemented() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        (workspace::NAME, workspace::FIELDS.as_slice()),
        (project::NAME, project::FIELDS.as_slice()),
        (event::NAME, event::FIELDS.as_slice()),
    ]
}
