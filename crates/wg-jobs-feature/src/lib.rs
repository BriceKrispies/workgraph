//! The jobs feature (docs/rust-rewrite.md 4.16, D15): the job and worker routes of the HTTP API
//! (IF-46: H080 to H084) over the job queue. The queue core is `wg-jobs`; the queue's writes run
//! through the application's runtime as the CLI's `job` commands do, so the rules are one copy.
//!
//! * [`spec`] (Pure): the route table and the page window of a job listing;
//! * [`service`] (Shell): the reads and the two writes (cancel, retry) over `&mut App`;
//! * [`http`] (Shell): the route table bound to the service.
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list. It has
//! no commands, tools, job handlers or maintenance steps.

pub mod http;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The jobs feature: its routes only.
pub static FEATURE: Feature = Feature {
    name: "jobs",
    cli: &[],
    routes: &[http::GROUP],
    tools: &[],
    resources: &[],
    jobs: &[],
    maintenance: &[],
};
