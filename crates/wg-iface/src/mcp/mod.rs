//! The MCP rules the features and the MCP server share (docs/rust-rewrite.md section 4.3, IF-30):
//!
//! * [`catalog`]: the tool, resource and resource-template descriptors from the embedded
//!   `contracts/mcp.json` snapshot, in the order the Python server registers them, in the
//!   camelCase form the wire carries;
//! * [`arguments`]: a tool's arguments checked against its input schema, with the Pydantic error
//!   text the Python SDK puts in an `isError` result;
//! * [`outcome`]: the `tools/call` result shapes and `tool_errors`' error mapping
//!   (`src/workgraph/mcp/server.py`);
//! * [`permissions`]: the agent's permission set from `--grant` flags, one function for both
//!   transports (WG-3734);
//! * [`options`]: the serve options and the calling agent.

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
