//! wg-registry: workspaces, projects, repositories and reference resolution.
//!
//! `Core` tier. The lookups are pure functions over the slices of `wg-model`; the shell loads the
//! rows a reference can name and maps the returned [`WgError`](wg_types::WgError) to its response.

pub mod boundary;
pub mod checkout;
pub mod health;
pub mod identity;
pub mod projects;
pub mod register;
pub mod remote;
pub mod renames;
pub mod resolve;
pub mod settings;
pub mod workspaces;
