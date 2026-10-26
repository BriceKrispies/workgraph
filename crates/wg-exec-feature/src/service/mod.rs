//! The exec feature's shell (Shell): the load-decide-apply glue over `&mut App`.
//!
//! * [`worktrees`]: the worktree service (`WorktreeManager` in `integrations/git/worktrees.py`):
//!   recovery, listing, release, removal and preparation of an attempt's worktrees.

pub mod worktrees;
