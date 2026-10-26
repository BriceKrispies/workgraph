//! The trackers feature (docs/rust-rewrite.md 4.16, D15): the external tracker items of tracked
//! tasks and the `tracker` commands. The Jira mapping and the sync decisions are the core crate
//! `wg-trackers`; this crate holds the statement text, the readers over a session, the connection
//! documents and the CLI group. Modules, one per tier:
//!
//! * [`spec`] (Pure): the statements and the row decoding, the connection and status documents,
//!   the clap command and the human views of the `tracker` commands;
//! * [`service`] (Shell): free functions over `&mut App` and `&mut Session`, the readers the store
//!   served and the entry points of the commands;
//! * [`cli`] (Shell): the clap group, a table of `fn` pointers into [`service`].
//!
//! The feature exports [`FEATURE`]: the `tracker` CLI group and the `tracker_sync` job. Its routes
//! and tools are not served yet.

pub mod cli;
pub mod service;
pub mod spec;

use wg_jobs::trackers::TRACKER_SYNC_KIND;
use wg_runtime::{Feature, JobKind};

/// The trackers feature: its `tracker` CLI group and the `tracker_sync` job. It has no routes,
/// tools or resources yet.
pub static FEATURE: Feature = Feature {
    name: "trackers",
    cli: &[cli::GROUP],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: &[JobKind {
        kind: TRACKER_SYNC_KIND,
        handler: service::sync::tracker_sync_job,
    }],
};
