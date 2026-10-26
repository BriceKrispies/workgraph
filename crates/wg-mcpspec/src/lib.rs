//! wg-mcpspec: the pure half of `wg-mcp` (docs/rust-rewrite.md section 4.3, IF-30).
//!
//! The MCP server is a transport (stdio, Streamable HTTP) wrapped around a handful of decisions
//! that are functions of values, and they live here so the shell crate keeps only the I/O:
//!
//! * [`catalog`]: the tool, resource and resource-template descriptors from the embedded
//!   `contracts/mcp.json` snapshot, in the order the Python server registers them, in the
//!   camelCase form the wire carries;
//! * [`arguments`]: a tool's arguments checked against its input schema, with the Pydantic error
//!   text the Python SDK puts in an `isError` result;
//! * [`outcome`]: the `tools/call` result shapes and `tool_errors`' error mapping
//!   (`src/workgraph/mcp/server.py`);
//! * [`permissions`]: the agent's permission set from `--grant` flags, one function for both
//!   transports (WG-3734).

pub mod arguments;
pub mod catalog;
pub mod options;
pub mod outcome;
pub mod permissions;
pub mod wire;

pub use arguments::{ArgumentsError, check_arguments};
pub use catalog::{Catalog, ToolSpec};
pub use outcome::{
    INVALID_PARAMS_MESSAGE, error_result, execution_error_text, has_params_to_parse, read_result,
    respond_to_call, structured_output, success_result, text_argument, tool_error_text,
    unknown_tool_text,
};
pub use permissions::{agent_permissions, require, tool_permissions};
pub use wire::structured;

/// The server name in `serverInfo`.
pub const SERVER_NAME: &str = "workgraph";

/// The server title in `serverInfo`.
pub const SERVER_TITLE: &str = "Workgraph";

/// The server version in `serverInfo` (`workgraph.__version__`).
pub const SERVER_VERSION: &str = "0.1.0";

/// The `instructions` of the `initialize` result.
pub const INSTRUCTIONS: &str = "Workgraph tracks engineering work across repositories.
Typical agent loop: task_get_ready -> task_claim -> attempt_create -> (work, committing with a
`Task-ID: <KEY>` trailer) -> task_report_progress -> task_link_commit -> task_report_result.
Reporting success moves a task to awaiting verification/review; only humans or configured
verification accept work. Task references accept a UUID or a key such as WG-12.";

/// The protocol version the server speaks (the recorded one).
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// The `initialize` result: protocol version, capabilities (nothing subscribes or changes, and
/// `prompts` is advertised although none exist, as the Python SDK does), server info and
/// instructions.
pub fn initialize_info() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "prompts": {"listChanged": false},
            "resources": {"listChanged": false, "subscribe": false},
            "tools": {"listChanged": false},
        },
        "serverInfo": {
            "name": SERVER_NAME,
            "title": SERVER_TITLE,
            "version": SERVER_VERSION,
        },
        "instructions": INSTRUCTIONS,
    })
}
