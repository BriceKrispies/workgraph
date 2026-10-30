//! The tasks feature (docs/rust-rewrite.md 4.16, D15): the task commands, acceptance, claims, links
//! and the task routes and tools, over the pure domain core `wg-tasks`. One vertical crate:
//!
//! * [`spec`] (Pure): request parsing, reply shaping, the DTOs and the human views;
//! * [`service`] (Shell): free functions over `&mut App`, the load-decide-apply glue;
//! * [`cli`], [`http`] and [`mcp`] (Shell): the clap group, the route table and the tool table.
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod http;
pub mod jobs;
pub mod mcp;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The tasks feature: its CLI group, its routes, its tools and its expired-claim maintenance step.
/// It has no job kinds. Its resource is [`mcp::RESOURCES`] (the task detail).
pub static FEATURE: Feature = Feature {
    name: "tasks",
    cli: &[cli::GROUP],
    routes: &[http::GROUP, http::ACCEPTANCE],
    tools: mcp::TOOLS,
    resources: mcp::RESOURCES,
    jobs: &[],
    maintenance: jobs::MAINTENANCE,
};
