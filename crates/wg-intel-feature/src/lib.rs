//! The intelligence feature (docs/rust-rewrite.md 4.16, D15): classification history, reuse,
//! gold labels and evaluations, the lexical search candidates, the search scope and the embedding
//! reads, and the `classify` and `search` commands. The pure classification, taxonomy, evaluation
//! and ranking code is the core crate `wg-intel`. Three modules, one per tier:
//!
//! * [`spec`] (Pure): the statements, the row decoding, the output shapes and the command arguments
//!   and views;
//! * [`service`] (Shell): the readers over `&mut Session` and the entry points over `&mut App`;
//! * [`cli`] (Shell): the clap groups `classify` and `search`, each a `fn`-pointer table into
//!   [`service`].
//!
//! The feature exports [`FEATURE`]. Its routes are [`http`] (H114 to H116); it has no tools or resources yet.

pub mod cli;
pub mod http;
pub mod jobs;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The intelligence feature: the `classify` and `search` command groups and the classification,
/// embedding and evaluation jobs.
pub static FEATURE: Feature = Feature {
    name: "intel",
    cli: &[cli::CLASSIFY, cli::SEARCH],
    routes: &[http::GROUP],
    tools: &[],
    resources: &[],
    jobs: jobs::JOBS,
    maintenance: &[],
};
