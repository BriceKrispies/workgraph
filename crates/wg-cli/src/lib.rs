//! wg-cli: the `workgraph` binary: clap command tree and handlers.
//!
//! The binary parses, calls one runtime entry point and renders; business rules live below
//! `wg-runtime`. This crate is the skeleton every command group builds on:
//!
//! * [`run`] is the entry point: `--json` is stripped from anywhere on the command line, the
//!   clap tree parses what is left, the matched command runs and a failure becomes the report
//!   and exit code of `cli/common.py` (`wg-clispec` holds the pure rules).
//! * [`wg_clispec::tree`] builds the tree from the `contracts/cli.json` snapshot. A group implemented in
//!   [`commands`] supplies its own clap command; every other command is a placeholder that parses
//!   like the real one and then reports that it is not implemented.
//! * [`CliState`] carries the global options, opens the [`wg_runtime::App`] on first use and
//!   prints results as JSON (`--json`) or through `wg-render`.

pub mod commands;
mod output;
mod run;

pub use output::{console, write_stderr, write_stdout};
pub use run::{run, run_with};
pub use wg_runtime::CliState;
