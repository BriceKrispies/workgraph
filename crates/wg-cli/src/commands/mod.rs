//! The command groups implemented so far, one module per group (`commands/<group>.rs`).
//!
//! A group is implemented once it is in [`GROUPS`] or in a feature's `cli` table listed in
//! [`FEATURES`]; `tests/if01_cli.rs` derives its implemented list from both and compares each
//! group's clap command against `contracts/cli.json`. A top-level command or group that is not
//! implemented is a placeholder built from the snapshot (see `wg_clispec::tree`).

pub mod job;
pub mod serve;
pub mod ui;
pub mod worker;

use wg_runtime::{CliGroup, CliRun, Feature};

/// What a handler receives: the global state, the command path below the root
/// (`["task", "list"]`) and the matches of the leaf command.
pub type Handler = CliRun;

/// One top-level command or group (the descriptor of `wg_runtime::feature`).
pub type Group = CliGroup;

/// The features of this binary, in the order their command groups are dispatched and listed as
/// implemented. A feature task adds its crate's `Feature` here.
pub const FEATURES: &[&Feature] = &[
    &wg_tasks_feature::FEATURE,
    &wg_decisions::FEATURE,
    &wg_registry_feature::FEATURE,
    &wg_automation_feature::FEATURE,
    &wg_git_feature::FEATURE,
];

/// The implemented groups that predate the feature descriptor, in CLI help order.
pub const GROUPS: &[Group] = &[
    serve::MCP,
    serve::SERVE,
    job::GROUP,
    ui::GROUP,
    worker::GROUP,
];
