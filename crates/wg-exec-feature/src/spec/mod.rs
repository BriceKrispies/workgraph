//! The exec feature's pure half (Pure): the managed worktree decisions as functions of values.
//!
//! * [`worktrees`]: crash recovery, the release and removal decisions, the preparation decisions
//!   and the outputs the worktree commands print.
//! * [`cli_attempt`]: the clap command of the `attempt` group and the human views of its reads.

pub mod cli_attempt;
pub mod worktrees;
