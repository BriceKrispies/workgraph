//! The events feature (docs/rust-rewrite.md 4.16, D15): the immutable domain event log as the CLI
//! reads it, `events list` (the history) and `events verify` (the replay of the task events). The
//! pure core is `wg-events` (upcasters, projection, verify); this crate holds the vertical slice of
//! the CLI, one module per tier:
//!
//! * [`spec`] (Pure): the clap command, the readers of its matches, the history query, the
//!   `--json` documents and the human views;
//! * [`service`] (Shell): free functions over `&mut App` that read the store and call `wg-events`;
//! * [`cli`] (Shell): the clap group, a [`wg_runtime::CliGroup`].
//!
//! The feature has no routes or tools yet (the history route is served by `wg-server`), so its
//! `routes` and `tools` tables are empty.
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The events feature: its CLI group, `events`.
pub static FEATURE: Feature = Feature {
    name: "events",
    cli: &[cli::GROUP],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: &[],
    maintenance: &[],
};
