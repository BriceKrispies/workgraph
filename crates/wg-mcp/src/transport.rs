//! The two transports.
//!
//! Both are built from the same [`Agent`], so `--grant` permissions reach stdio and the `/mcp/`
//! mount of `wg-server` alike (WG-3734). There is no HTTP transport of its own: the endpoint is
//! only served behind `wg-server`'s bearer-token and Host/Origin checks (WG-3764).

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderValue, Method, Request, Response, StatusCode, header};
use axum::middleware::{self, Next};
use rmcp::ServiceExt;
use rmcp::transport::common::server_side_http::session_id;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService, stdio};
use tokio_util::sync::CancellationToken;
use wg_mcpspec::not_jsonrpc_reply;
use wg_types::{ErrorCode, WgError};

use crate::backend::{Agent, ToolBackend};
use crate::handler::WgMcp;

/// The largest request body the endpoint reads before answering.
const BODY_LIMIT: usize = 4 * 1024 * 1024;

fn failure(what: &str, err: impl std::fmt::Display) -> WgError {
    WgError::new(ErrorCode::WorkgraphError, format!("{what}: {err}"))
}

/// Serves one client over this process's stdin and stdout until it closes the pipe.
pub async fn serve_stdio(agent: Agent, backend: Box<dyn ToolBackend>) -> Result<(), WgError> {
    let server = WgMcp::new(agent, backend)?;
    let running = server
        .serve(stdio())
        .await
        .map_err(|err| failure("mcp stdio", err))?;
    running
        .waiting()
        .await
        .map_err(|err| failure("mcp stdio", err))?;
    Ok(())
}

/// The Streamable HTTP service for `agent`: one handler per session.
pub fn http_service(
    agent: Agent,
    backend: Box<dyn ToolBackend>,
    cancel: CancellationToken,
) -> Result<StreamableHttpService<WgMcp, LocalSessionManager>, WgError> {
    let server = WgMcp::new(agent, backend)?;
    let mut config = StreamableHttpServerConfig::default();
    config.cancellation_token = cancel;
    // The server's own middleware checks Host and Origin (loopback and the paired LAN
    // addresses); the SDK's loopback-only default would refuse the paired devices.
    config.allowed_hosts = Vec::new();
    Ok(StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    ))
}

/// The router `wg-server` merges into its own: the MCP endpoint under `/mcp` (axum serves `/mcp/`
/// too). A POST whose body is not a JSON-RPC message is answered `400` here, as the Python
/// server does (X6), before the SDK's own request checks run.
pub fn http_router(
    agent: Agent,
    backend: Box<dyn ToolBackend>,
    cancel: CancellationToken,
) -> Result<Router, WgError> {
    let service = http_service(agent, backend, cancel)?;
    Ok(Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn(reject_not_jsonrpc)))
}

/// The X6 check: a POST body that is not a JSON-RPC message gets the `400` JSON-RPC error,
/// carrying a session id as the Python transport does; every other request passes through.
async fn reject_not_jsonrpc(request: Request<Body>, next: Next) -> Response<Body> {
    if request.method() != Method::POST {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, BODY_LIMIT).await else {
        return plain(StatusCode::PAYLOAD_TOO_LARGE, "request body too large");
    };
    let Some(reply) = not_jsonrpc_reply(&bytes) else {
        return next
            .run(Request::from_parts(parts, Body::from(bytes)))
            .await;
    };
    let Ok(text) = serde_json::to_vec(&reply) else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "cannot encode the reply");
    };
    let mut response = Response::new(Body::from(text));
    *response.status_mut() = StatusCode::BAD_REQUEST;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Ok(id) = HeaderValue::from_str(&session_id()) {
        headers.insert("mcp-session-id", id);
    }
    response
}

fn plain(status: StatusCode, text: &'static str) -> Response<Body> {
    let mut response = Response::new(Body::from(text));
    *response.status_mut() = status;
    response
}
