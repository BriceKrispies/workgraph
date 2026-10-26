//! The trackers feature (docs/rust-rewrite.md 4.16, D15): the external tracker items of tracked
//! tasks. The Jira mapping and the sync decisions are the core crate `wg-trackers`; this crate
//! holds the statement text and the readers over a session. Two modules, one per tier:
//!
//! * [`spec`] (Pure): the statements and the row decoding;
//! * [`service`] (Shell): free functions over `&mut Session`, the readers the store served.
//!
//! The feature exports [`FEATURE`]. Nothing is served yet, so its tables are empty.

pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The trackers feature. It has no CLI group, routes, tools, resources or jobs yet.
pub static FEATURE: Feature = Feature {
    name: "trackers",
    cli: &[],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: &[],
};
