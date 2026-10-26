//! What a tool call or a resource read runs: the application, under the agent's identity.

use serde_json::Value;
pub use wg_mcpspec::options::Agent;
use wg_types::{ErrorCode, WgError};

/// The application behind the server. The server holds it behind one lock, so a call has the
/// application to itself (`App` is `&mut`: one SQLite connection, one decision at a time). Calls
/// arrive with validated arguments, defaults filled in, and run on a blocking thread.
pub trait ToolBackend: Send + 'static {
    /// Runs tool `name`; the value is its structured output, an error becomes an `isError` result.
    fn call_tool(&mut self, agent: &Agent, name: &str, arguments: Value) -> Result<Value, WgError>;

    /// Reads resource `uri`; the text is the JSON document.
    fn read_resource(&mut self, agent: &Agent, uri: &str) -> Result<String, WgError>;
}

/// The backend over `wg_runtime::App`. The tools are wired by the interface tasks that follow
/// the skeleton, so every call is refused for now.
pub struct AppBackend(pub wg_runtime::App);

impl ToolBackend for AppBackend {
    fn call_tool(
        &mut self,
        _agent: &Agent,
        name: &str,
        _arguments: Value,
    ) -> Result<Value, WgError> {
        Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("tool {name} is not wired yet"),
        ))
    }

    fn read_resource(&mut self, _agent: &Agent, uri: &str) -> Result<String, WgError> {
        Err(WgError::new(
            ErrorCode::NotFound,
            format!("resource {uri} is not wired yet"),
        ))
    }
}
