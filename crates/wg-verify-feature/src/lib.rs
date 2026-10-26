//! The verification feature (docs/rust-rewrite.md 4.16, D15): check configs of one scope, check
//! runs, their results and the failure history. The check configs, selection, outcomes and
//! acceptance rules are the core crate `wg-verify`; this crate holds the statement text and the
//! readers over a session. Two modules, one per tier:
//!
//! * [`spec`] (Pure): the statements, the filters and pages, and the row decoding;
//! * [`service`] (Shell): free functions over `&mut Session`, the readers the store served.
//!
//! The reads base code shares (the workspace configs, the newest run, the task acceptances) stay
//! in `wg-store`. The feature exports [`FEATURE`]. Nothing is served yet, so its tables are empty.

pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The verification feature. It has no CLI group, routes, tools, resources or jobs yet.
pub static FEATURE: Feature = Feature {
    name: "verify",
    cli: &[],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: &[],
};
