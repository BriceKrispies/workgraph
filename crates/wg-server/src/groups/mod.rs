//! The API route groups (plan section 9.9).
//!
//! A group task adds a module here with `pub fn routes() -> Vec<ApiRoute>` for its operations,
//! chains it into [`routes`] (which `ServerOptions::routes` takes), and adds its group name
//! (`wg_serverspec::operations::group_of`) to [`IMPLEMENTED`]. The route-table test then requires
//! every operation of the group to be registered as the OpenAPI snapshot declares it; groups not
//! listed are reported, not failed (plan 5.5).
//!
//! A handler parses nothing itself: the path and query parameters arrive validated against the
//! snapshot, and the body against the route's [`crate::BodySpec`]. It calls the runtime through
//! [`crate::Server::blocking`] and returns an [`crate::Answer`].

pub mod events;
pub mod meta_settings;

use crate::route::ApiRoute;

/// The groups whose routes are registered.
pub const IMPLEMENTED: &[&str] = &["meta-settings", "IF-49"];

/// Every registered API route, from the implemented groups.
pub fn routes() -> Vec<ApiRoute> {
    let mut table = Vec::new();
    meta_settings::register(&mut table);
    events::register(&mut table);
    table
}
