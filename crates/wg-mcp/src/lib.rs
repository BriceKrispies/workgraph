//! wg-mcp: MCP server over stdio and Streamable HTTP.
//!
//! The decisions that are functions of values (descriptors, argument validation, error text,
//! `--grant` permissions, the flags) live in `wg-mcpspec`. This crate is the I/O: it runs
//! `rmcp`'s transports around a [`handler::WgMcp`] that answers from the snapshot descriptors and
//! hands each validated tool call to a [`backend::ToolBackend`].
//!
//! * [`serve_stdio`] and [`serve_http`] run one transport each;
//! * [`http_router`] is the `/mcp/` router `wg-server` mounts, built from the same
//!   [`backend::Agent`] (the same `--grant` permissions) as the stdio transport (WG-3734).

pub mod backend;
mod decision_tools;
pub mod handler;
mod run_tools;
pub mod tools;
pub mod transport;

pub use backend::{Agent, AppBackend, ToolBackend};
pub use handler::WgMcp;
pub use transport::{http_router, serve_http, serve_stdio};
