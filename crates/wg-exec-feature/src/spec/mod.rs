//! The exec feature's pure half (Pure): the managed worktree decisions as functions of values.
//!
//! * [`worktrees`]: crash recovery, the release and removal decisions, the preparation decisions
//!   and the outputs the worktree commands print.
//! * [`cli_attempt`]: the clap command of the `attempt` group and the human views of its reads.
//! * [`cli_run`]: the `run` clap tree and the documents and output of its `get` and `watch`.
//! * [`attempts`]: the attempt rows of the run flow (new, status change, finish).
//! * [`runs`]: the run start, cancel and retry decisions; [`prompt`]: the agent's instruction.
//! * [`schedule`] and [`decompose`]: the scheduler and the child proposal; [`sql`]: the statements
//!   they read with.

pub mod artifacts;
pub mod attempt_writes;
pub mod attempts;
pub mod cli_agent;
pub mod cli_attempt;
pub mod cli_run;
pub mod conclude;
pub mod config;
pub mod decompose;
pub mod execute;
pub mod gate;
pub mod outcome;
pub mod prompt;
pub mod runs;
pub mod schedule;
pub mod sql;
pub mod worktrees;
