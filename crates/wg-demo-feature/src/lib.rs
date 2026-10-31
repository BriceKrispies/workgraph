//! The demo feature (docs/rust-rewrite.md 4.16, D15): `demo e2e`, the offline cross-repository
//! demonstration in an isolated home, and `demo seed`, the demo data for the web UI (C063, C064).
//! The scripted agent and the demo checks are the `workgraph-demo-agent` binary (DV4), not Python.
//!
//! * [`spec`] (Pure): the clap command, the readers of its matches, the demo repositories and
//!   their files, the seed table, the scripts the repositories carry and the human views;
//! * [`service`] (Shell): the steps that touch the store and Git, through the feature services of
//!   the other crates;
//! * [`cli`] (Shell): the clap group, a [`wg_runtime::CliGroup`].
//!
//! The feature has no routes, tools or jobs: the demonstration's HTTP and MCP steps drive the
//! current executable (`serve` and `mcp`) as child processes.
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The demo feature: its CLI group, `demo`.
pub static FEATURE: Feature = Feature {
    name: "demo",
    cli: &[cli::GROUP],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: &[],
    maintenance: &[],
};
