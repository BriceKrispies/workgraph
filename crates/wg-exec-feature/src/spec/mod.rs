//! The exec feature's pure half (Pure): the managed worktree decisions as functions of values.
//!
//! * [`worktrees`]: crash recovery, the release and removal decisions, the preparation decisions
//!   and the outputs the worktree commands print.
//! * [`cli_run`]: the `run` clap tree and the documents and output of its `get` and `watch`.

pub mod cli_run;
pub mod worktrees;
