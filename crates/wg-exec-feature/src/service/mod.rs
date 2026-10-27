//! The exec feature's shell (Shell): the load-decide-apply glue over `&mut App`.
//!
//! * [`worktrees`]: the worktree service (`WorktreeManager` in `integrations/git/worktrees.py`):
//!   recovery, listing, release, removal and preparation of an attempt's worktrees.
//! * [`attempts`]: the attempt reads (`AttemptService.list`, `get`, `manifests`, `compare`).
//! * [`resolve`]: the task and attempt reference resolvers the attempt tools and commands share.
//! * [`runs`]: the run reads of the `run` commands (`AgentRunService.get`).
//! * [`run_flow`]: queueing, cancelling, retrying and rerunning managed runs.
//! * [`agents`]: the agent adapters a run is launched through; [`agent_cmds`]: the `agent`
//!   commands over the configuration.

pub mod agent_cmds;
pub mod agents;
pub mod attempt_runs;
pub mod attempts;
pub mod conclude;
pub mod execute;
pub mod outcome;
pub mod resolve;
pub mod run_flow;
pub mod runs;
pub mod scheduler;
pub mod worktrees;
