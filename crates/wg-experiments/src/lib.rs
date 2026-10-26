//! wg-experiments: protocol validation, planner, trials, statistics, predictor.
//!
//! This crate is `Core` tier (docs/rust-rewrite.md section 4.6): the decisions are pure
//! functions with immutable bindings, iterator chains and folds, and the floating-point
//! operation order follows the Python source (`src/workgraph/experiments/`) so that results
//! match it bit for bit (plan risk R5).

pub mod briefs;
pub mod contract;
pub mod escape;
pub mod pool;
pub mod predictor;
pub mod replay;
pub mod stats;
