//! The two transports.
//!
//! Both are built from the same [`Agent`], so `--grant` permissions reach stdio and the `/mcp/`
//! mount of `wg-server` alike (WG-3734). There is no HTTP transport of its own: the endpoint is
//! only served behind `wg-server`'s bearer-token and Host/Origin checks (WG-3764).

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::http::request::Parts;
use axum::http::{HeaderValue, Method, Request, Response, StatusCode, header};
use axum::middleware::{self, Next};
use rmcp::ServiceExt;
use rmcp::transport::common::server_side_http::session_id;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService, stdio};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use wg_mcpspec::{Gate, Reject, SDK_ACCEPT, Verb, gate};
use wg_types::{ErrorCode, WgError};

use crate::backend::{Agent, ToolBackend};
use crate::handler::WgMcp;

/// The session id header of the Streamable HTTP transport.
const MCP_SESSION_ID: &str = "mcp-session-id";

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
/// too), behind the Python endpoint's own checks in the order the Python transport makes them.
pub fn http_router(
    agent: Agent,
    backend: Box<dyn ToolBackend>,
    cancel: CancellationToken,
) -> Result<Router, WgError> {
    let service = http_service(agent, backend, cancel)?;
    Ok(Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn(check_request)))
}

/// The checks the Python endpoint makes before the SDK's own. A GET or POST whose `Accept` header
/// is refused gets the `406` JSON-RPC error (`wg_mcpspec::accept`). An accepted request has its
/// `Accept` widened to what the SDK checks for. A POST whose body is not a JSON-RPC message then
/// gets the `400` (X6). Every other request passes through.
async fn check_request(request: Request<Body>, next: Next) -> Response<Body> {
    let verb = match *request.method() {
        Method::POST => Verb::Post,
        Method::GET => Verb::Get,
        _ => return next.run(request).await,
    };
    let accept = request
        .headers()
        .get(header::ACCEPT)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
        .unwrap_or_default();
    // The session id the Python transport echoes: the request's own, or a fresh one for the
    // session that a request without one would open.
    let session = request
        .headers()
        .get(MCP_SESSION_ID)
        .and_then(|value| value.to_str().ok())
        .map_or_else(|| session_id().to_string(), str::to_owned);
    let (parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, BODY_LIMIT).await else {
        return plain(StatusCode::PAYLOAD_TOO_LARGE, "request body too large");
    };
    match gate(verb, &accept, &bytes) {
        Gate::Reply { reject, body } => json_reply(reject_status(reject), &body, &session),
        Gate::Forward => forward(next, parts, bytes).await,
    }
}

/// Hands an accepted request to the SDK, its `Accept` header widened to what the SDK checks for.
async fn forward(next: Next, mut parts: Parts, bytes: Bytes) -> Response<Body> {
    parts
        .headers
        .insert(header::ACCEPT, HeaderValue::from_static(SDK_ACCEPT));
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

/// The HTTP status of a refusal the endpoint answers itself.
fn reject_status(reject: Reject) -> StatusCode {
    match reject {
        Reject::NotAcceptable => StatusCode::NOT_ACCEPTABLE,
        Reject::BadRequest => StatusCode::BAD_REQUEST,
    }
}

/// A JSON reply carrying the session id header, as the Python transport writes its errors.
fn json_reply(status: StatusCode, body: &Value, session: &str) -> Response<Body> {
    let Ok(text) = serde_json::to_vec(body) else {
        return plain(StatusCode::INTERNAL_SERVER_ERROR, "cannot encode the reply");
    };
    let mut response = Response::new(Body::from(text));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Ok(id) = HeaderValue::from_str(session) {
        headers.insert(MCP_SESSION_ID, id);
    }
    response
}

fn plain(status: StatusCode, text: &'static str) -> Response<Body> {
    let mut response = Response::new(Body::from(text));
    *response.status_mut() = status;
    response
}
