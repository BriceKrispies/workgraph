//! The exec feature (docs/rust-rewrite.md 4.16, D15, FC-07): managed worktrees and the attempt and
//! run tools, over the pure exec core `wg-exec` (attempt state machine, run flow, manifests,
//! worktree and branch planning). One vertical crate:
//!
//! * [`spec`] (Pure): the worktree decisions (recovery, release, removal, preparation) and the
//!   outputs the worktree commands print;
//! * [`service`] (Shell): the worktree service as free functions over `&mut App`, the load-decide-
//!   apply glue;
//! * [`mcp`] (Shell): the attempt and run tool table;
//! * [`cli`] (Shell): the `run` command group (`get` and `watch`), a handler table into [`service`].
//!
//! The git-facing worktree commands (`git worktrees`, `git cleanup-worktrees`) still belong to the
//! git group and are answered by `wg-runtime`'s git service until the git feature moves (FC-06).
//! The feature has no routes or jobs yet: the attempt group is not ported, the `run` group's
//! `start`, `cancel`, `retry`, `rerun`, `schedule` and `decompose` are not, and the worker's
//! recovery pass reaches the worktree service through the caller.
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod mcp;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The exec feature: its `run` command group and its tools. It has no routes or jobs yet.
pub static FEATURE: Feature = Feature {
    name: "exec",
    cli: &[cli::GROUP],
    routes: &[],
    tools: mcp::TOOLS,
    resources: &[],
    jobs: &[],
    maintenance: &[],
};
