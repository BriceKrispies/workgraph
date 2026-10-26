//! The registry feature's command groups: `workspace` and `project` (IF-03), `repo` (IF-04) and
//! `config`, `doctor` and `init` (IF-02). Each handler reads its options, calls one [`crate::service`]
//! entry point and prints the result with [`crate::spec`]'s views.

pub mod registry;
pub mod repo;
pub mod setup;
