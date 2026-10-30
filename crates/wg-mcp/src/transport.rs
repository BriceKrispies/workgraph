//! The two transports.
//!
//! Both are built from the same [`Agent`], so `--grant` permissions reach stdio and the `/mcp/`
//! mount of `wg-server` alike (WG-3734). There is no HTTP transport of its own: the endpoint is
//! only served behind `wg-server`'s bearer-token and Host/Origin checks (WG-3764).

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{HeaderName, HeaderValue, Request, Response, StatusCode, header};
use axum::middleware::{self, Next};
use futures_util::StreamExt;
use rmcp::ServiceExt;
use rmcp::service::ServerInitializeError;
use rmcp::transport::common::server_side_http::session_id;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{SessionId, SessionManager};
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService, stdio};
use tokio_util::sync::CancellationToken;
use wg_mcpspec::{
    Plan, Reply, Step, Touch, check_body, check_headers, frame_events, too_large, touch_up, verb_of,
};
use wg_types::WgError;

use crate::backend::{Agent, ToolBackend};
use crate::handler::WgMcp;

/// The largest request body the endpoint reads before answering.
const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// The header that names a session.
const SESSION_HEADER: &str = "mcp-session-id";

/// Serves one client over this process's stdin and stdout until it closes the pipe. A pipe that
/// closes before `initialize` is a clean end, as the Python server treats it (exit 0, WG-3821).
pub async fn serve_stdio(agent: Agent, backend: Box<dyn ToolBackend>) -> Result<(), WgError> {
    let server = WgMcp::new(agent, backend)?;
    let running = match server.serve(stdio()).await {
        Ok(running) => running,
        Err(ServerInitializeError::ConnectionClosed(_)) => return Ok(()),
        Err(err) => return Err(WgError::workgraph(format!("mcp stdio: {err}"))),
    };
    running
        .waiting()
        .await
        .map_err(|err| WgError::workgraph(format!("mcp stdio: {err}")))?;
    Ok(())
}

/// The Streamable HTTP service for `agent`, with the session table it keeps: one handler per
/// session.
fn http_parts(
    agent: Agent,
    backend: Box<dyn ToolBackend>,
    cancel: CancellationToken,
) -> Result<
    (
        StreamableHttpService<WgMcp, LocalSessionManager>,
        Arc<LocalSessionManager>,
    ),
    WgError,
> {
    let server = WgMcp::new(agent, backend)?;
    let mut config = StreamableHttpServerConfig::default();
    config.cancellation_token = cancel;
    // The server's own middleware checks Host and Origin (loopback and the paired LAN
    // addresses); the SDK's loopback-only default would refuse the paired devices.
    config.allowed_hosts = Vec::new();
    let sessions = Arc::new(LocalSessionManager::default());
    let service =
        StreamableHttpService::new(move || Ok(server.clone()), Arc::clone(&sessions), config);
    Ok((service, sessions))
}

/// The Streamable HTTP service for `agent`: one handler per session.
pub fn http_service(
    agent: Agent,
    backend: Box<dyn ToolBackend>,
    cancel: CancellationToken,
) -> Result<StreamableHttpService<WgMcp, LocalSessionManager>, WgError> {
    Ok(http_parts(agent, backend, cancel)?.0)
}

/// The router `wg-server` merges into its own: the MCP endpoint under `/mcp` (axum serves `/mcp/`
/// too), behind the Python endpoint's own checks in the order the Python transport makes them.
pub fn http_router(
    agent: Agent,
    backend: Box<dyn ToolBackend>,
    cancel: CancellationToken,
) -> Result<Router, WgError> {
    let (service, sessions) = http_parts(agent, backend, cancel)?;
    Ok(Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(sessions, check_request)))
}

/// The checks the Python endpoint makes before the SDK's own; `wg_mcpspec` decides. The glue reads
/// the request (its body first, under the size limit, as the Python body-limit middleware does for
/// every method) and applies the decision.
async fn check_request(
    State(sessions): State<Arc<LocalSessionManager>>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let verb = verb_of(request.method().as_str());
    let (mut parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, BODY_LIMIT).await else {
        return write(too_large());
    };
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .map(|(name, value)| {
            let text = String::from_utf8_lossy(value.as_bytes()).into_owned();
            (name.as_str().to_owned(), text)
        })
        .collect();
    let session_known = match parts
        .headers
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        Some(id) => {
            let key: SessionId = id.into();
            sessions.has_session(&key).await.unwrap_or(false)
        }
        None => false,
    };
    let fresh = session_id();
    let plan = match check_headers(verb, &headers, session_known, &fresh) {
        Step::Answer(plan) => plan,
        Step::ReadBody => check_body(&headers, &fresh, &bytes),
    };
    match plan {
        Plan::Reply(reply) => write(reply),
        Plan::Forward { accept } => {
            let session = parts
                .headers
                .get(SESSION_HEADER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            parts
                .headers
                .insert(header::ACCEPT, HeaderValue::from_static(accept));
            let response = next
                .run(Request::from_parts(parts, Body::from(bytes)))
                .await;
            let touch = touch_up(verb, response.status().as_u16(), &session);
            touched(python_framed(response), touch)
        }
    }
}

/// Applies the endpoint's changes to the SDK's response.
fn touched(mut response: Response<Body>, touch: Touch) -> Response<Body> {
    if let Some(status) = touch.status {
        *response.status_mut() = StatusCode::from_u16(status).unwrap_or(response.status());
    }
    touch.headers.iter().for_each(|(name, value)| {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            response.headers_mut().insert(name, value);
        }
    });
    response
}

/// Writes a planned reply.
fn write(reply: Reply) -> Response<Body> {
    let mut response = Response::new(Body::from(reply.body));
    *response.status_mut() = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::BAD_REQUEST);
    reply.headers.iter().for_each(|(name, value)| {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            response.headers_mut().insert(name, value);
        }
    });
    response
}

/// An event stream from the SDK, rewritten event by event to the Python framing as its bytes
/// arrive. Any other response passes through.
fn python_framed(response: Response<Body>) -> Response<Body> {
    let is_events = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/event-stream"));
    if !is_events {
        return response;
    }
    let (parts, body) = response.into_parts();
    let mut pending: Vec<u8> = Vec::new();
    let stream = body.into_data_stream().map(move |chunk| {
        chunk.map(|bytes| {
            pending.extend_from_slice(&bytes);
            let (out, used) = frame_events(&pending);
            pending.drain(..used);
            out
        })
    });
    Response::from_parts(parts, Body::from_stream(stream))
}
