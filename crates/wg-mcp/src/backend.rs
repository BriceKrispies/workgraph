//! What a tool call or a resource read runs: the application, under the agent's identity.

use serde_json::Value;
pub use wg_mcpspec::options::Agent;
use wg_runtime::{App, Feature};
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

/// The backend over `wg_runtime::App`. The tools and resources of `features` are asked in feature
/// order, after the attempt and run tools; the rest are refused until their interface tasks land.
/// A caller with no features passes `features: &[]`.
pub struct AppBackend {
    pub app: App,
    pub features: &'static [&'static Feature],
}

impl ToolBackend for AppBackend {
    fn call_tool(&mut self, agent: &Agent, name: &str, arguments: Value) -> Result<Value, WgError> {
        // Dispatch: each tool group answers the names it owns. The IF-33 attempt, run and git
        // tools come first (M47 to M56), then the features' tools.
        if let Some(result) = crate::run_tools::call(&mut self.app, agent, name, &arguments) {
            return result;
        }
        let feature_tools = self
            .features
            .iter()
            .flat_map(|feature| feature.tools.iter());
        for tool in feature_tools {
            if let Some(answer) = tool(&mut self.app, agent, name, &arguments) {
                return answer;
            }
        }
        Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("tool {name} is not wired yet"),
        ))
    }

    fn read_resource(&mut self, agent: &Agent, uri: &str) -> Result<String, WgError> {
        let feature_resources = self
            .features
            .iter()
            .flat_map(|feature| feature.resources.iter());
        for resource in feature_resources {
            if let Some(text) = resource(&mut self.app, agent, uri) {
                return text;
            }
        }
        Err(WgError::new(
            ErrorCode::NotFound,
            format!("resource {uri} is not wired yet"),
        ))
    }
}
