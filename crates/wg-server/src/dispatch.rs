//! One request, from the socket to the response: the security screen, routing, validation, the
//! handler and the security headers.

use std::sync::Arc;

use crate::spec::pages::{OAUTH2_REDIRECT_HTML, SWAGGER_HTML};
use crate::spec::pairing::{pair_form, pair_submit};
use crate::spec::routes::{Found, find};
use crate::spec::security::{Incoming, screen};
use crate::spec::spa::{SpaStep, file_reply, spa_page, spa_step};
use crate::spec::{Target, request_context};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::Method;
use axum::response::Response;
use tower::ServiceExt;
use wg_iface::http::form::parse_qsl;
use wg_iface::http::net::Ip;
use wg_iface::http::reply::{
    JSON_TYPE, Reply, body_parse_error, error_reply, method_not_allowed, secured, secured_headers,
    to_http,
};
use wg_iface::http::request::check_request;
use wg_runtime::new_id;
use wg_runtime::spec::{GETUSER_VARIABLES, getuser};

use crate::route::{Answer, ApiRequest};
use crate::server::Server;

/// The application as an axum router: every request goes through [`dispatch`].
pub fn router(server: Arc<Server>) -> Router {
    Router::new().fallback(dispatch).with_state(server)
}

/// What a route produced: a reply, or a response built elsewhere (a stream, the MCP transport).
enum Produced {
    Reply(Reply),
    Response(Response),
}

/// The application: `LocalSecurityMiddleware` around the FastAPI app.
async fn dispatch(State(server): State<Arc<Server>>, request: Request) -> Response {
    let client = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|info| match info.0.ip() {
            std::net::IpAddr::V4(v4) => Ip::V4(u32::from(v4)),
            std::net::IpAddr::V6(v6) => Ip::V6(u128::from(v6)),
        });
    let (parts, body) = request.into_parts();
    let incoming = Incoming::from_parts(&parts, client);
    let head = parts.method == Method::HEAD;
    if let Some(rejected) = screen(&server.policy, &incoming) {
        return to_http(rejected, head).map(Body::from);
    }
    let produced = match find(&server.table, &incoming.method, &incoming.path) {
        Found::Route { target, params } => match *target {
            Target::OpenApi => Produced::Reply(Reply::new(200, JSON_TYPE, server.openapi.clone())),
            Target::Docs => Produced::Reply(Reply::html(200, SWAGGER_HTML)),
            Target::OAuth2Redirect => Produced::Reply(Reply::html(200, OAUTH2_REDIRECT_HTML)),
            Target::PairForm => Produced::Reply(pair_form(&server.policy, &incoming)),
            Target::PairSubmit => Produced::Reply(match to_bytes(body, usize::MAX).await {
                Ok(bytes) => {
                    server.with_pairing(|state| pair_submit(&server.policy, state, &bytes))
                }
                Err(_) => body_parse_error(),
            }),
            Target::Spa => Produced::Reply(spa(&server, &incoming, &params)),
            Target::Mcp => match &server.mcp {
                Some(mcp) => Produced::Response(
                    mcp.clone()
                        .oneshot(Request::from_parts(parts, body))
                        .await
                        .unwrap_or_else(|never| match never {}),
                ),
                None => Produced::Reply(Reply::new(
                    404,
                    "text/plain; charset=utf-8",
                    b"Not Found".to_vec(),
                )),
            },
            Target::Api(index) => api(&server, index, params, &incoming, parts.headers, body).await,
        },
        Found::NotAllowed(allow) => Produced::Reply(method_not_allowed(&allow)),
        Found::Missing => Produced::Reply(Reply::new(
            404,
            "text/plain; charset=utf-8",
            b"Not Found".to_vec(),
        )),
    };
    match produced {
        Produced::Reply(reply) => to_http(secured(reply, &incoming.path), head).map(Body::from),
        Produced::Response(response) => {
            let (mut parts, body) = response.into_parts();
            parts.headers = secured_headers(std::mem::take(&mut parts.headers), &incoming.path);
            Response::from_parts(parts, body)
        }
    }
}

/// The SPA route (X7): a file of the UI, else the page.
fn spa(server: &Server, incoming: &Incoming, params: &[(String, String)]) -> Reply {
    let path = params
        .first()
        .map(|(_, value)| value.as_str())
        .unwrap_or("");
    let page = || {
        let index = server
            .ui
            .file("index.html")
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        spa_page(&server.policy, incoming, index.as_deref())
    };
    match spa_step(path) {
        SpaStep::NotFound(reply) => reply,
        SpaStep::File(relative) => match server.ui.file(&relative) {
            Some(bytes) => file_reply(&relative, bytes),
            None => page(),
        },
        SpaStep::Page => page(),
    }
}

/// An API route: validation, the request context, the handler.
async fn api(
    server: &Arc<Server>,
    index: usize,
    params: Vec<(String, String)>,
    incoming: &Incoming,
    headers: axum::http::HeaderMap,
    body: Body,
) -> Produced {
    let Some((route, declared)) = server.routes.get(index) else {
        return Produced::Reply(wg_iface::http::reply::internal_error());
    };
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return Produced::Reply(body_parse_error()),
    };
    let checked = match check_request(
        declared,
        &params,
        &parse_qsl(&incoming.query),
        &bytes,
        header("content-type"),
    ) {
        Ok(checked) => checked,
        Err(reply) => return Produced::Reply(reply),
    };
    let logins: Vec<Option<String>> = GETUSER_VARIABLES
        .iter()
        .map(|key| std::env::var(key).ok())
        .collect();
    // The two headers are read as bytes: Python reads any byte, not only UTF-8 text.
    let raw = |name: &str| headers.get(name).map(|value| value.as_bytes());
    let context = new_id().and_then(|fresh| {
        request_context(
            raw("x-workgraph-actor"),
            raw("x-correlation-id"),
            getuser(&logins),
            fresh,
        )
    });
    let context = match context {
        Ok(context) => context,
        Err(error) => return Produced::Reply(error_reply(&error)),
    };
    let request = ApiRequest {
        params: checked.params,
        body: checked.body,
        given: checked.given,
        context,
        client: incoming.client,
        headers,
    };
    match (route.handler)(Arc::clone(server), request).await {
        Ok(Answer::Json(value)) => Produced::Reply(Reply::json(route.status, &value)),
        Ok(Answer::Reply(reply)) => Produced::Reply(reply),
        Ok(Answer::Stream(response)) => Produced::Response(response),
        Err(error) => Produced::Reply(error_reply(&error)),
    }
}
