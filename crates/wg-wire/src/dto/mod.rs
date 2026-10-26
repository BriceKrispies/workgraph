//! The DTOs, one module per area.
//!
//! Each DTO module exports `NAME` and `FIELDS` and a `from_model` constructor; see the crate
//! documentation for the conventions. Later wire tasks add their modules here and their DTOs to
//! [`implemented`].

pub mod project;
pub mod stack;
pub mod stack_create;
pub mod stack_item;
pub mod stack_item_add;
pub mod stack_repository;
pub mod workspace;

/// The DTOs that exist: the OpenAPI component schema name and the field names in order.
pub fn implemented() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        (workspace::NAME, workspace::FIELDS.as_slice()),
        (project::NAME, project::FIELDS.as_slice()),
        (stack::NAME, stack::FIELDS.as_slice()),
        (stack_item::NAME, stack_item::FIELDS.as_slice()),
        (stack_repository::NAME, stack_repository::FIELDS.as_slice()),
        (stack_create::NAME, stack_create::FIELDS.as_slice()),
        (stack_item_add::NAME, stack_item_add::FIELDS.as_slice()),
    ]
}
