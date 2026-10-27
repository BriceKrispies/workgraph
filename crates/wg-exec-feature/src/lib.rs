//! The exec feature (docs/rust-rewrite.md 4.16, D15, FC-07): managed worktrees, the attempt and
//! run tools and the run flow, over the pure exec core `wg-exec` (attempt state machine, run flow,
//! manifests, worktree and branch planning). One vertical crate:
//!
//! * [`spec`] (Pure): the worktree decisions (recovery, release, removal, preparation), the run
//!   flow's decisions (start, cancel, conclude, the scheduler, the child proposal) and the outputs
//!   the commands print;
//! * [`service`] (Shell): the worktree service and the run flow as free functions over `&mut App`,
//!   the load-decide-apply glue; agents are launched through its `AgentHost` trait;
//! * [`jobs`] (Shell): the `agent_run` and `decompose_task` job handlers and the scheduler pass of
//!   the maintenance plan;
//! * [`mcp`] (Shell): the attempt and run tool table;
//! * [`cli`] (Shell): the `run` command group, a handler table into [`service`].
//!
//! The git-facing worktree commands (`git worktrees`, `git cleanup-worktrees`) still belong to the
//! git group and are answered by `wg-runtime`'s git service until the git feature moves (FC-06).
//! The feature has no routes yet. Concluding a run reaches the
//! features that do not serve it yet (commit attribution, stack readiness, verification runs)
//! through `service::conclude::Collaborators`.
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod jobs;
pub mod mcp;
pub mod service;
pub mod spec;

use wg_runtime::{CliGroup, Feature};

/// The exec feature's command groups: `attempt`, `agent` and `run`.
pub const CLI: &[CliGroup] = &[cli::attempt::GROUP, cli::agent::GROUP, cli::run::GROUP];

/// The exec feature: its command groups, its tools, its job handlers and its maintenance step. It
/// has no routes or resources yet.
pub static FEATURE: Feature = Feature {
    name: "exec",
    cli: CLI,
    routes: &[],
    tools: mcp::TOOLS,
    resources: &[],
    jobs: jobs::JOBS,
    maintenance: jobs::MAINTENANCE,
};
