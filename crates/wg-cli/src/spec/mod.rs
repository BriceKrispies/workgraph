//! The pure rules only the `workgraph` binary uses, as `Pure` modules of `wg-cli`: the job and
//! worker option readers, the `mcp` and `serve` option readers and the `ui` option reader.
//! What the command groups of the features share is `wg_iface::cli`.

pub mod job;
pub mod servers;
pub mod ui;
