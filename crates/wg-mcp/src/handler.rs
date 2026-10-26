//! The `rmcp` handler: listings from the catalog, calls through the backend.
//!
//! Everything the server holds (catalog, agent, application) lives in one [`Session`] behind a
//! lock: a request has the application to itself, as `App` is one SQLite connection. Each
//! request takes the session through [`with_session`], which also reads the answer into the
//! `rmcp` model type.

use std::sync::{Arc, Mutex};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CustomRequest, CustomResult, ErrorCode,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;
use wg_mcpspec::{
    Catalog, INVALID_PARAMS_MESSAGE, has_params_to_parse, initialize_info, read_result,
    respond_to_call,
};

use crate::backend::{Agent, ToolBackend};

/// The state one server shares between requests.
pub struct Session {
    catalog: Catalog,
    agent: Agent,
    backend: Box<dyn ToolBackend>,
}

/// One MCP server: cheap to clone, one clone per HTTP session.
#[derive(Clone)]
pub struct WgMcp {
    session: Arc<Mutex<Session>>,
}

/// Runs `answer` with the session and reads its wire value as the model type `T`. A request that
/// panicked while holding the session poisons it for good.
fn with_session<T: serde::de::DeserializeOwned>(
    session: &Mutex<Session>,
    answer: impl FnOnce(&mut Session) -> Result<Value, ErrorData>,
) -> Result<T, ErrorData> {
    let mut guard = session.lock().map_err(|_| {
        ErrorData::internal_error("the server state is poisoned by an earlier failure", None)
    })?;
    let value = answer(&mut guard)?;
    serde_json::from_value(value)
        .map_err(|err| ErrorData::internal_error(format!("descriptor: {err}"), None))
}

impl WgMcp {
    /// A server for `agent` over `backend`, with the embedded catalog.
    pub fn new(agent: Agent, backend: Box<dyn ToolBackend>) -> Result<Self, wg_types::WgError> {
        Ok(Self {
            session: Arc::new(Mutex::new(Session {
                catalog: Catalog::embedded()?,
                agent,
                backend,
            })),
        })
    }
}

impl ServerHandler for WgMcp {
    fn get_info(&self) -> ServerConfig {
        with_session(&self.session, |_| Ok(initialize_info())).unwrap_or_default()
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        with_session(&self.session, |s| Ok(s.catalog.tools_list()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let session = Arc::clone(&self.session);
        let name = request.name.to_string();
        let arguments = request.arguments.map(Value::Object);
        let result: rmcp::model::CallToolResult = tokio::task::spawn_blocking(move || {
            with_session(&session, |s| {
                let Session {
                    catalog,
                    agent,
                    backend,
                } = s;
                Ok(respond_to_call(
                    catalog,
                    &name,
                    arguments.as_ref(),
                    |valid| backend.call_tool(agent, &name, valid),
                ))
            })
        })
        .await
        .map_err(|err| ErrorData::internal_error(err.to_string(), None))??;
        Ok(result.into())
    }

    /// A request for a method the server has, whose params did not parse, lands here (the typed
    /// parse failed); the SDK's answer is `-32602` with an empty `data` string.
    async fn on_custom_request(
        &self,
        request: CustomRequest,
        _context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        if has_params_to_parse(&request.method) {
            return Err(ErrorData::invalid_params(
                INVALID_PARAMS_MESSAGE,
                Some(Value::String(String::new())),
            ));
        }
        Err(ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            request.method,
            None,
        ))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        with_session(&self.session, |s| Ok(s.catalog.resources_list()))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        with_session(&self.session, |s| Ok(s.catalog.resource_templates_list()))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let session = Arc::clone(&self.session);
        let uri = request.uri;
        let result: rmcp::model::ReadResourceResult = tokio::task::spawn_blocking(move || {
            with_session(&session, |s| {
                let Session { agent, backend, .. } = s;
                backend
                    .read_resource(agent, &uri)
                    .map(|text| read_result(&uri, &text))
                    .map_err(|err| ErrorData::resource_not_found(err.message, None))
            })
        })
        .await
        .map_err(|err| ErrorData::internal_error(err.to_string(), None))??;
        Ok(result.into())
    }
}
