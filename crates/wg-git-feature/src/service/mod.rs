//! The Shell of the git feature: the load-decide-apply glue over `&mut App` (the entry points of
//! the `git` commands, and `git sync`) and over `&mut Session` (the store reads of [`store`]).
//!
//! The entry points are free functions because an inherent `impl` of `App` cannot be written in
//! this crate; they are reached from the feature's [`crate::cli`] table.

pub mod store;

mod attempt_links;
mod entries;
mod sync;

pub use attempt_links::link_attempt_commits;
pub use entries::*;
pub(crate) use sync::sync_repository;
pub use sync::{git_sync, git_sync_ref};
