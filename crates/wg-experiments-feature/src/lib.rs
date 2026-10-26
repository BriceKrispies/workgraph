//! The experiments feature (docs/rust-rewrite.md 4.16, D15): experiments, their trials and the
//! predictor fits. The pure statistics and the pool are the core crate `wg-experiments`; this
//! crate holds the statement text, the readers over a session and the `experiment` commands. Its
//! modules, one per tier:
//!
//! * [`spec`] (Pure): the statements, the row decoding and the human views;
//! * [`service`] (Shell): the readers over `&mut Session`, and the entry points over `&mut App`;
//! * [`cli`] (Shell): the clap group.
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The experiments feature: its CLI group (`experiment list, show`). It has no routes, tools,
/// resources or jobs yet.
pub static FEATURE: Feature = Feature {
    name: "experiments",
    cli: &[cli::GROUP],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: &[],
};
