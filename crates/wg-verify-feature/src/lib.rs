//! The verification feature (docs/rust-rewrite.md 4.16, D15): check configs of one scope, check
//! runs, their results, the failure history and the acceptance a tracker's done issue implies. The
//! check configs, selection, outcomes and acceptance rules are the core crate `wg-verify`; this
//! crate holds the statement text, the readers over a session, the listings and the `verify`
//! command group. The modules, one per tier:
//!
//! * [`spec`] (Pure): the statements, the filters and pages, the row decoding, the external
//!   acceptance decision, and the `verify` clap tree and tables;
//! * [`service`] (Shell): free functions over `&mut Session` (the readers the store served), the
//!   listings over `&mut App`, and the external acceptance that takes the caller's session;
//! * [`cli`] (Shell): the `verify` group, a table of handlers into [`service`].
//!
//! The reads base code shares (the workspace configs, the newest run, the task acceptances) stay
//! in `wg-store`. The feature exports [`FEATURE`]. Not yet served: `verify config` and `verify
//! run` (see [`cli`]), the routes, the tools and the jobs.

pub mod cli;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The verification feature: its `verify` command group. It has no routes, tools, resources or
/// jobs yet.
pub static FEATURE: Feature = Feature {
    name: "verify",
    cli: &[cli::GROUP],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: &[],
    maintenance: &[],
};
