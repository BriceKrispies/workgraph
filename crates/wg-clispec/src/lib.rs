//! wg-clispec: the pure half of the `workgraph` binary (docs/rust-rewrite.md sections 4.3, 5.5
//! and DV1).
//!
//! `wg-cli` parses, calls one runtime entry point and renders. Everything here that does not touch
//! a stream, the environment or clap is a function of its inputs: the `--json` rule for argv, the
//! command snapshot (`contracts/cli.json`) and the comparison of a command tree against it, the
//! clap tree built from it, the exit-code mapping and the text of error reports (contracts O02 and O05).

pub mod argv;
pub mod automation;
pub mod decision;
pub mod failure;
pub mod registry;
pub mod servers;
pub mod setup;
pub mod snapshot;
pub mod task;
pub mod tree;
pub mod ui;

pub use argv::{Argv, is_hook_invocation, split_json};
pub use failure::{CliFailure, EXIT_INTERRUPTED, EXIT_UNEXPECTED, Report, report};
pub use snapshot::{
    CommandSpec, Kind, ParamSpec, Snapshot, SnapshotError, diff_commands, display_path,
    not_implemented_message,
};
