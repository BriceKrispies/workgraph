//! The command groups implemented so far, one module per group (`commands/<group>.rs`).
//!
//! A group is implemented once it is in [`GROUPS`] or in a feature's `cli` table listed in
//! [`FEATURES`]; `tests/if01_cli.rs` derives its implemented list from both and compares each
//! group's clap command against `contracts/cli.json`. A top-level command or group that is not
//! implemented is a placeholder built from the snapshot (see `wg_clispec::tree`).

pub mod automation;
pub mod decision;
pub mod git;
pub mod job;
pub mod registry;
pub mod repo;
pub mod serve;
pub mod setup;
pub mod task;
pub mod ui;
pub mod worker;

use wg_runtime::{CliGroup, CliRun, Feature};

/// What a handler receives: the global state, the command path below the root
/// (`["task", "list"]`) and the matches of the leaf command.
pub type Handler = CliRun;

/// One top-level command or group (the descriptor of `wg_runtime::feature`).
pub type Group = CliGroup;

/// The features of this binary, in the order their command groups are dispatched and listed as
/// implemented. A feature task adds its crate's `Feature` here; none is listed yet.
pub const FEATURES: &[&Feature] = &[];

/// The implemented groups that predate the feature descriptor, in CLI help order.
pub const GROUPS: &[Group] = &[
    repo::GROUP,
    setup::CONFIG,
    setup::DOCTOR,
    setup::INIT,
    serve::MCP,
    serve::SERVE,
    decision::GROUP,
    job::GROUP,
    git::GROUP,
    automation::GROUP,
    task::GROUP,
    ui::GROUP,
    registry::PROJECT,
    registry::WORKSPACE,
    worker::GROUP,
];
