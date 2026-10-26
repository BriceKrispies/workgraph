//! The task tools (`mcp/server.py`). The Rust tool layer answers no task tool yet: a task tool
//! name reaches the generic `wg-mcp` handling, as before this feature moved, so [`TOOLS`] answers
//! nothing. The tools are listed in `wg-mcpspec`'s catalog and are added here when their bodies
//! are ported.

use serde_json::Value;
use wg_mcpspec::options::Agent;
use wg_runtime::{App, ToolRun};
use wg_types::WgError;

/// The feature's tool descriptor: a call that is never one of its own tools.
pub const TOOLS: &[ToolRun] = &[call];

/// Runs tool `name` for `agent`, or returns `None` when the tool is not a task tool.
pub fn call(
    _app: &mut App,
    _agent: &Agent,
    _name: &str,
    _arguments: &Value,
) -> Option<Result<Value, WgError>> {
    None
}
