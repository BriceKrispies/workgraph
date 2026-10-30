//! wg-iface: what the three interfaces (CLI, HTTP, MCP) share with the features that target them
//! (docs/rust-rewrite.md section 4.16, D15).
//!
//! A feature crate contributes tables of `fn` pointers to the front ends, so the types those
//! tables name, and the readers and shapers its handlers use, sit below both: the front ends
//! (`wg-cli`, `wg-server`, `wg-mcp`) and the features depend on this crate, and it depends on
//! neither. Everything only one front end uses is a `Pure` module of that front end instead.

pub mod cli;
pub mod http;
pub mod mcp;
