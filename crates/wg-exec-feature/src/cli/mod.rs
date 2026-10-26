//! The exec feature's command groups: `attempt` (IF-09). Each handler reads its options, calls one
//! [`crate::service`] entry point and prints the result with [`crate::spec`]'s views.

pub mod attempt;
