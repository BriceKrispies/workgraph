//! The stacks feature (docs/rust-rewrite.md 4.16, D15, FC-08): a parent task's children integrated
//! on one integration branch per repository, verified, bisected, ejected and landed. The pure
//! rules (membership, queue, ejection, bisection, landing decisions) are the core crate
//! `wg-stacks`; this crate holds the whole vertical slice, one module per tier:
//!
//! * [`spec`] (Pure): the clap tree of the `stack` group, the human views of its results, the
//!   stack commands as decisions over the task world, the executor's transactions
//!   ([`spec::exec`]) and the statement text of the stack readers;
//! * [`service`] (Shell): free functions over `&mut App`, the load-decide-apply glue of the stack
//!   services (`create`, `show`, `list`, `add`, `remove`, `readmit`, `integrate`, `verify`,
//!   `history`) and the executor ([`service::exec`]: the integrate and verify jobs, the rebuild of
//!   integration branches, `eject`, and the one implementation of landing a stack);
//! * [`cli`] (Shell): the clap group, a dispatch over [`service`] and [`spec::view`];
//! * [`http`] (Shell): the `/stacks` routes (H062 to H070);
//! * [`mcp`] (Shell): the stack tools (M09 to M16), each a call of [`service`] under the agent;
//! * [`jobs`] (Shell): the `stack_integrate` and `stack_verify` job handlers.
//!
//! Base crates do not depend on this crate; the tasks feature calls
//! [`service::exec::land_stack_accepted`] when it accepts a stack's parent.
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod http;
pub mod jobs;
pub mod mcp;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The stacks feature: its CLI group, routes, tools and job handlers.
pub static FEATURE: Feature = Feature {
    name: "stacks",
    cli: &[cli::GROUP],
    routes: &[http::GROUP],
    tools: mcp::TOOLS,
    resources: &[],
    jobs: jobs::KINDS,
    maintenance: &[],
};
