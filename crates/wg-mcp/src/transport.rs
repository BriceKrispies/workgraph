//! The two transports.
//!
//! Both are built from the same [`Agent`], so `--grant` permissions reach stdio and the `/mcp/`
//! mount of `wg-server` alike (WG-3734). There is no HTTP transport of its own: the endpoint is
//! only served behind `wg-server`'s bearer-token and Host/Origin checks (WG-3764).

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{HeaderName, HeaderValue, Request, Response, StatusCode, header};
use axum::middleware::{self, Next};
use rmcp::ServiceExt;
use rmcp::transport::common::server_side_http::session_id;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService, stdio};
use tokio_util::sync::CancellationToken;
use wg_mcpspec::{Plan, plan, verb_of};
use wg_types::WgError;

use crate::backend::{Agent, ToolBackend};
use crate::handler::WgMcp;

/// The largest request body the endpoint reads before answering.
const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// Serves one client over this process's stdin and stdout until it closes the pipe.
pub async fn serve_stdio(agent: Agent, backend: Box<dyn ToolBackend>) -> Result<(), WgError> {
    let server = WgMcp::new(agent, backend)?;
    let running = server
        .serve(stdio())
        .await
        .map_err(|err| WgError::workgraph(format!("mcp stdio: {err}")))?;
    running
        .waiting()
        .await
        .map_err(|err| WgError::workgraph(format!("mcp stdio: {err}")))?;
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

/// The checks the Python endpoint makes before the SDK's own; `wg_mcpspec::plan` decides. The
/// glue reads the request, applies the plan and passes every method but GET and POST through.
async fn check_request(request: Request<Body>, next: Next) -> Response<Body> {
    let Some(verb) = verb_of(request.method().as_str()) else {
        return next.run(request).await;
    };
    let (mut parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, BODY_LIMIT).await else {
        let mut response = Response::new(Body::from("request body too large"));
        *response.status_mut() = StatusCode::PAYLOAD_TOO_LARGE;
        return response;
    };
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .map(|(name, value)| {
            let text = String::from_utf8_lossy(value.as_bytes()).into_owned();
            (name.as_str().to_owned(), text)
        })
        .collect();
    match plan(verb, &headers, &session_id(), &bytes) {
        Plan::Reply {
            status,
            headers,
            body,
        } => reply(status, &headers, body),
        Plan::Forward { accept } => {
            *parts
                .headers
                .entry(header::ACCEPT)
                .or_insert_with(|| HeaderValue::from_static(accept)) =
                HeaderValue::from_static(accept);
            next.run(Request::from_parts(parts, Body::from(bytes)))
                .await
        }
    }
}

/// Writes a planned reply.
fn reply(status: u16, headers: &[(String, String)], body: Vec<u8>) -> Response<Body> {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST);
    headers.iter().for_each(|(name, value)| {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            response.headers_mut().insert(name, value);
        }
    });
    response
}
