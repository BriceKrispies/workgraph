//! The exec feature's shell (Shell): the load-decide-apply glue over `&mut App`.
//!
//! * [`worktrees`]: the worktree service (`WorktreeManager` in `integrations/git/worktrees.py`):
//!   recovery, listing, release, removal and preparation of an attempt's worktrees.
//! * [`attempts`]: the attempt reads (`AttemptService.list`, `get`, `manifests`, `compare`).
//! * [`resolve`]: the task and attempt reference resolvers the attempt tools and commands share.

pub mod agents;
pub mod attempt_runs;
pub mod attempts;
pub mod conclude;
pub mod resolve;
pub mod worktrees;
