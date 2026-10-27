//! The stacks feature (docs/rust-rewrite.md 4.16, D15): a parent task's children integrated on one
//! integration branch per repository, verified, bisected, ejected and landed. The stack services
//! (`create`, `show`, `list`, `add`, `remove`, `readmit`, `integrate`, `verify`, `history`) are the
//! application's (`wg_runtime::App`, IF-53), and the ejection and membership rules are the core
//! crate `wg-stacks`; this crate holds the vertical slice of the CLI group, one module per tier:
//!
//! * [`spec`] (Pure): the clap tree of the `stack` group (from the snapshot) and the human views of
//!   its results;
//! * [`service`] (Shell): free functions over `&mut App`, the load-decide-apply glue of the commands
//!   the application does not serve yet (`eject`);
//! * [`cli`] (Shell): the clap group, a dispatch over [`service`] and [`spec::view`];
//! * [`mcp`] (Shell): the stack tools (M09 to M16), each a call of [`service`] under the agent.
//!
//! The landing implementation (`stack land`, the rebuild of integration branches) moves into this
//! crate with its own feature task; until then this crate calls what the application provides.
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod mcp;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The stacks feature: its CLI group and its tools. It has no routes, resources or jobs of its own;
/// the `/stacks` routes and the integration and verification jobs are served by the application.
pub static FEATURE: Feature = Feature {
    name: "stacks",
    cli: &[cli::GROUP],
    routes: &[],
    tools: mcp::TOOLS,
    resources: &[],
    jobs: &[],
    maintenance: &[],
};
