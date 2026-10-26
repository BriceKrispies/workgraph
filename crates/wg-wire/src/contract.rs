//! The contract snapshots, embedded.
//!
//! `contracts/openapi.json` is the OpenAPI document FastAPI generated and `contracts/mcp.json` the
//! MCP tool, resource and resource-template descriptors. The server and the MCP interface serve
//! these bytes' content instead of generating it, so the documents cannot drift from the
//! snapshots (plan 4.8).

use serde_json::Value;
use wg_types::{ErrorCode, WgError};

/// The OpenAPI document, as recorded.
pub const OPENAPI_JSON: &str = include_str!("../../../contracts/openapi.json");

/// The MCP descriptors, as recorded.
pub const MCP_JSON: &str = include_str!("../../../contracts/mcp.json");

fn parse(name: &str, text: &str) -> Result<Value, WgError> {
    serde_json::from_str(text).map_err(|err| {
        WgError::new(
            ErrorCode::WorkgraphError,
            format!("embedded {name} snapshot is not valid JSON: {err}"),
        )
    })
}

/// The OpenAPI document with object key order as recorded.
pub fn openapi() -> Result<Value, WgError> {
    parse("openapi", OPENAPI_JSON)
}

/// The MCP descriptors: `tools`, `resources` and `resource_templates`.
pub fn mcp() -> Result<Value, WgError> {
    parse("mcp", MCP_JSON)
}

/// The `components.schemas` object of the OpenAPI document.
pub fn component_schemas() -> Result<serde_json::Map<String, Value>, WgError> {
    let document = openapi()?;
    document
        .get("components")
        .and_then(|components| components.get("schemas"))
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| {
            WgError::new(
                ErrorCode::WorkgraphError,
                "embedded openapi snapshot has no components.schemas",
            )
        })
}

/// The MCP tool descriptors, in snapshot order.
pub fn mcp_tools() -> Result<Vec<Value>, WgError> {
    let document = mcp()?;
    document
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| {
            WgError::new(
                ErrorCode::WorkgraphError,
                "embedded mcp snapshot has no tools",
            )
        })
}
