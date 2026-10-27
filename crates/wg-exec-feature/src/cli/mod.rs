//! The exec feature's command groups: `attempt` (IF-09), `agent` and `run` (IF-10). Each handler
//! reads its options, calls one [`crate::service`] entry point and prints the result with
//! [`crate::spec`]'s views.

pub mod agent;
pub mod attempt;
pub mod run;
