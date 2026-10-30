//! The analytics feature (docs/rust-rewrite.md 4.16, D15): the source extraction for the analytics
//! warehouse (SH-14). The fact tables, metrics and query engine are the core crate `wg-analytics`;
//! this crate holds the table copies and the readers over a session. Two modules, one per tier:
//!
//! * [`spec`] (Pure): the statements, the table list and the copy's DTO;
//! * [`service`] (Shell): free functions over `&mut Session`, the readers the store served.
//!
//! The feature exports [`FEATURE`]. Nothing is served yet, so its tables are empty.

pub mod cli;
pub mod http;
pub mod mcp;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The analytics feature: its CLI group (`metrics`, `query`, ...) and its routes ([`http`]).
/// Its tools and resources are [`mcp`] (M59 to M63, the catalog and the metric resources); it has
/// no jobs yet.
pub static FEATURE: Feature = Feature {
    name: "analytics",
    cli: &[cli::GROUP],
    routes: &[http::GROUP],
    tools: mcp::TOOLS,
    resources: mcp::RESOURCES,
    jobs: &[],
    maintenance: &[],
};
