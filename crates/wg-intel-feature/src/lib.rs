//! The intelligence feature (docs/rust-rewrite.md 4.16, D15): classification history, reuse,
//! gold labels and evaluations, the lexical search candidates, the search scope and the embedding
//! reads. The pure classification, taxonomy, evaluation and ranking code is the core crate
//! `wg-intel`; this crate holds the statement text and the readers over a session. Two modules,
//! one per tier:
//!
//! * [`spec`] (Pure): the statements and the row decoding;
//! * [`service`] (Shell): free functions over `&mut Session`, the readers the store served.
//!
//! The feature exports [`FEATURE`]. Nothing is served yet, so its tables are empty.

pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The intelligence feature. It has no CLI group, routes, tools, resources or jobs yet.
pub static FEATURE: Feature = Feature {
    name: "intel",
    cli: &[],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: &[],
    maintenance: &[],
};
