//! The two transports.
//!
//! Both are built from the same [`Agent`], so `--grant` permissions reach stdio, `mcp --transport
//! http` and the `/mcp/` mount of `wg-server` alike (WG-3734).

use std::sync::Arc;

use axum::Router;
use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService, stdio};
use tokio_util::sync::CancellationToken;
use wg_types::{ErrorCode, WgError};

use crate::backend::{Agent, ToolBackend};
use crate::handler::WgMcp;

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
/// too).
pub fn http_router(
    agent: Agent,
    backend: Box<dyn ToolBackend>,
    cancel: CancellationToken,
) -> Result<Router, WgError> {
    let service = http_service(agent, backend, cancel)?;
    Ok(Router::new().nest_service("/mcp", service))
}

/// Serves the endpoint on `listener` (already bound by the caller) until `cancel` fires.
pub async fn serve_http(
    listener: std::net::TcpListener,
    agent: Agent,
    backend: Box<dyn ToolBackend>,
    cancel: CancellationToken,
) -> Result<(), WgError> {
    let router = http_router(agent, backend, cancel.clone())?;
    listener
        .set_nonblocking(true)
        .map_err(|err| failure("mcp http", err))?;
    let listener =
        tokio::net::TcpListener::from_std(listener).map_err(|err| failure("mcp http", err))?;
    axum::serve(listener, router)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await
        .map_err(|err| failure("mcp http", err))
}
