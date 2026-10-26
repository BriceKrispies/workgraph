//! The registry entry points of the application: the services behind the `workspace`, `project`,
//! `repo`, `config`, `doctor` and `init` commands, the `/settings` routes and the registry tools.
//!
//! Each one resolves the workspace, loads the rows its decision reads in the transaction of the
//! call, hands them to the pure functions of [`crate::spec`] and applies what they return. These
//! are free functions over `&mut App` (and `&mut Session` in [`reads`]): the feature's Shell,
//! reached from the `cli`, `http` and `mcp` tables.

pub mod reads;
pub mod registry;
pub mod repos;
pub mod setup;

pub use registry::*;
pub use repos::*;
pub use setup::*;
