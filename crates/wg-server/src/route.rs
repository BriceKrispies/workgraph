//! API routes: what a route group registers and what its handlers receive and return.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::response::Response;
use serde_json::{Map, Value as Json};
use wg_runtime::RequestContext;
use wg_serverspec::net::Ip;
use wg_serverspec::reply::Reply;
use wg_serverspec::request::BodySpec;
use wg_types::WgError;

use crate::server::Server;

/// What a handler returns on success.
pub enum Answer {
    /// A JSON value, sent with the route's success status.
    Json(wg_json::Value),
    /// A complete reply (its own status and headers).
    Reply(Reply),
    /// A streaming response, such as [`crate::event_stream`].
    Stream(Response),
}

/// The future a handler returns.
pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<Answer, WgError>> + Send>>;

/// A route handler: the server and the validated request. A `WgError` becomes the error
/// envelope with the status of its code.
pub type Handler = Arc<dyn Fn(Arc<Server>, ApiRequest) -> HandlerFuture + Send + Sync>;

/// Wraps an `async fn(Arc<Server>, ApiRequest) -> Result<Answer, WgError>` as a [`Handler`].
pub fn handler<F, Fut>(f: F) -> Handler
where
    F: Fn(Arc<Server>, ApiRequest) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Answer, WgError>> + Send + 'static,
{
    Arc::new(move |server, request| -> HandlerFuture { Box::pin(f(server, request)) })
}

/// One API route of the OpenAPI document (an H id of the inventory).
#[derive(Clone)]
pub struct ApiRoute {
    /// The inventory id, `H001` to `H130`.
    pub id: &'static str,
    pub method: &'static str,
    /// The path template, exactly as in the snapshot (`/api/v1/ws/{ws}/tasks/{ref}`).
    pub path: &'static str,
    /// The success status (`status_code=` of the FastAPI route).
    pub status: u16,
    /// The query parameter names the route reads. Path and query values are validated against
    /// the snapshot's schemas; this list is what the route-table test compares.
    pub query: &'static [&'static str],
    /// The JSON body, when the route takes one.
    pub body: Option<BodySpec>,
    pub handler: Handler,
}

/// A request that passed the security screen and validation.
pub struct ApiRequest {
    /// The path and query parameters, validated, by name (defaults filled in).
    pub params: Map<String, Json>,
    /// The validated body, `null` when the route takes none or it was absent.
    pub body: Json,
    /// The body as the client sent it (`null` when absent), for the routes that tell a field left
    /// out from one set to null.
    pub given: Json,
    /// `deps.get_ctx`: the local operator as `human:<name>`, named by `X-Workgraph-Actor` (else
    /// the login name), with `X-Correlation-Id` as its correlation text when that header is at
    /// most 64 bytes (a fresh UUID otherwise, as Python's `get_ctx`).
    pub context: RequestContext,
    /// The peer address.
    pub client: Option<Ip>,
    /// The request headers, for the few routes that read one.
    pub headers: axum::http::HeaderMap,
}
