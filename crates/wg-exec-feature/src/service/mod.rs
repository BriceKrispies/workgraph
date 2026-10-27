//! The exec feature's shell (Shell): the load-decide-apply glue over `&mut App`.
//!
//! * [`worktrees`]: the worktree service (`WorktreeManager` in `integrations/git/worktrees.py`):
//!   recovery, listing, release, removal and preparation of an attempt's worktrees.
//! * [`runs`]: the run reads of the `run` commands (`AgentRunService.get`).
//! * [`run_flow`]: queueing, cancelling, retrying and rerunning managed runs.
//! * [`agents`]: the agent adapters a run is launched through.

pub mod agents;
pub mod run_flow;
pub mod runs;
pub mod scheduler;
pub mod worktrees;
