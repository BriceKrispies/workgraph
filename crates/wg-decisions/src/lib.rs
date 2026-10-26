//! The decisions feature (docs/rust-rewrite.md 4.16, D15): the questions a task waits on, the
//! options on offer and the answer. One vertical crate, one module per tier:
//!
//! * [`core`] (Core): request, choose and withdraw as pure plans over the task world (the plans of
//!   `core/services/decisions.py`);
//! * [`spec`] (Pure): request parsing, reply shaping, the DTOs, the SQL text and the human views;
//! * [`service`] (Shell): free functions over `&mut App` and `&mut Session`, the load-decide-apply
//!   glue;
//! * [`cli`], [`http`] and [`mcp`] (Shell): the clap group, the route table and the tool table,
//!   each a table of `fn` pointers into [`service`].
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod core;
pub mod http;
pub mod mcp;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The decisions feature: its CLI group, its routes and its tools. It has no resources or jobs.
pub static FEATURE: Feature = Feature {
    name: "decisions",
    cli: &[cli::GROUP],
    routes: &[http::GROUP],
    tools: mcp::TOOLS,
    resources: &[],
    jobs: &[],
};
