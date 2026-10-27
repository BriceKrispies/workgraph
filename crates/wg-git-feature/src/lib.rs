//! The git feature (docs/rust-rewrite.md 4.16, D15): the `git` command group (`sync`, `log`,
//! `show`, `link`, `unlink`, `changes`, `diff`, `surface`, `analyze`, `worktrees`,
//! `cleanup-worktrees`, `install-hook`), the commit ingestion behind `sync`, and the attribution of
//! commits to tasks. The pure core is `wg-gitcore` (parsers, attribution, rewrite
//! reconciliation); the git processes are `wg-git`. This crate is the feature above `wg-runtime`:
//!
//! * [`spec`] (Pure): the decisions and JSON shaping (ingestion, links, reads, analysis, worktree
//!   listing), the SQL text of the reads, the clap command and human views of the group;
//! * [`service`] (Shell): free functions over `&mut App` and `&mut Session`, the load-decide-apply
//!   glue;
//! * [`cli`] (Shell): the clap group, a table of `fn` pointers into [`service`].
//!
//! * [`jobs`]: the `git_sync` job handler, which the worker runs as `GitService.sync` does.
//!
//! The feature has no routes, tools or resources yet, so its tables are empty; the feature exports
//! [`FEATURE`] and the binary collects it into its ordered feature list.

pub mod cli;
pub mod jobs;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The git feature: its `git` command group and its `git_sync` job. It has no routes, tools,
/// resources or maintenance steps.
pub static FEATURE: Feature = Feature {
    name: "git",
    cli: &[cli::GROUP],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: jobs::JOBS,
    maintenance: &[],
};
