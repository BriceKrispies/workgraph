//! The pure half of the registry feature (Pure tier): the request and reply rules of the
//! workspace, project, repository, settings and setup services, the DTOs, the SQL text, the output
//! shapes, and the clap commands and human views of the `workspace`, `project`, `repo`, `config`,
//! `doctor` and `init` commands.
//!
//! The clap command and argument readers of the feature are `cli_*` (the snapshot's command,
//! `wg_clispec`'s argument helpers, the views); the logic is `registry`, `repos`, `settings` and
//! `setup`.

pub mod cli_registry;
pub mod cli_repo;
pub mod cli_setup;
pub mod dto;
pub mod mcp_shapes;
pub mod registry;
pub mod repos;
pub mod settings;
pub mod setup;
pub mod shapes;
pub mod sql;
