//! wg-serverspec: the pure half of `wg-server` (docs/rust-rewrite.md section 4.8).
//!
//! The HTTP server reproduces the FastAPI application of `api/app.py` and the middleware of
//! `api/security.py`. Everything in that work that is a function of values lives here, so the
//! shell crate keeps only the sockets, the file reads, the locks and the calls into the runtime:
//!
//! * [`security`]: the request screen (LAN clients, `Host`, `Origin`, the token), the device
//!   cookie, the allowed hosts and origins;
//! * [`pairing`]: pairing codes, the lockout and the `/pair` page;
//! * [`coverage`]: the route-table check against the OpenAPI snapshot;
//! * [`routes`] and [`operations`]: Starlette's route matching (first full match, else 405 from
//!   the first partial match) and the 130 operations in registration order with their groups;
//! * [`request`] and [`pyjson`]: FastAPI's parameter and body validation and its 422 and 400
//!   bodies, including Python's JSON decode errors;
//! * [`reply`]: responses as values and the security headers (O07);
//! * [`spa`] and [`pages`]: the single-page app route, the Swagger pages and the placeholder;
//! * [`sse`]: sse-starlette's event framing and ping (O08);
//! * [`net`], [`form`] and [`text`]: `ipaddress`, `urllib.parse` and `repr`/`html.escape`.

pub mod coverage;
pub mod form;
pub mod groups;
pub mod net;
pub mod operations;
pub mod pages;
pub mod pairing;
pub mod pyjson;
pub mod reply;
pub mod request;
pub mod routes;
pub mod security;
pub mod spa;
pub mod sse;
pub mod text;

use wg_types::{ErrorCode, WgError};

use crate::routes::Entry;

/// What a route of the application does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `GET /api/openapi.json` (X1).
    OpenApi,
    /// `GET /api/docs` (X2).
    Docs,
    /// `GET /docs/oauth2-redirect` (X3).
    OAuth2Redirect,
    /// The registered API route at this index.
    Api(usize),
    /// The MCP transport mounted at `/mcp` (X6).
    Mcp,
    /// `GET /pair` (X4).
    PairForm,
    /// `POST /pair` (X5).
    PairSubmit,
    /// `GET /{path:path}` (X7).
    Spa,
}

/// The route table in Starlette's order: FastAPI's own pages, the API routes (`(method, path)`
/// in registration order), the MCP mount when there is one, the pairing routes and the SPA
/// catch-all.
pub fn route_table(api: &[(&str, &str)], with_mcp: bool) -> Vec<Entry<Target>> {
    let mut entries = vec![
        Entry::page("/api/openapi.json", Target::OpenApi),
        Entry::page("/api/docs", Target::Docs),
        Entry::page("/docs/oauth2-redirect", Target::OAuth2Redirect),
    ];
    entries.extend(
        api.iter()
            .enumerate()
            .map(|(index, (method, path))| Entry::api(method, path, Target::Api(index))),
    );
    if with_mcp {
        entries.push(Entry::mount("/mcp", Target::Mcp));
    }
    entries.push(Entry::api("GET", "/pair", Target::PairForm));
    entries.push(Entry::api("POST", "/pair", Target::PairSubmit));
    entries.push(Entry::api("GET", "/{path:path}", Target::Spa));
    entries
}

/// The body of `GET /api/openapi.json`: the embedded snapshot as `JSONResponse` prints it.
pub fn openapi_body() -> Result<Vec<u8>, WgError> {
    let document = wg_wire::contract::openapi()?;
    wg_json::http_json(&wg_wire::convention::json_value(&document))
        .map(String::into_bytes)
        .map_err(|err| WgError::new(ErrorCode::WorkgraphError, format!("openapi: {err:?}")))
}

/// A header's value as Starlette reads it: each byte is one character (Latin-1), so the text of
/// a UTF-8 header is its bytes as characters, and any byte is accepted.
pub fn header_text(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// `X-Workgraph-Actor`: the acting human's name, cut to 100 characters (`deps.get_ctx`).
pub fn actor_name(header: Option<&[u8]>) -> Option<String> {
    header
        .filter(|name| !name.is_empty())
        .map(|name| header_text(name).chars().take(100).collect())
}

/// `X-Correlation-Id`: the text kept when it is at most 64 characters (Python's `len` of the
/// Latin-1 text, which is the byte count).
pub fn correlation_header(header: Option<&[u8]>) -> Option<String> {
    header
        .filter(|id| !id.is_empty() && id.len() <= 64)
        .map(header_text)
}

/// `deps.get_ctx(request)`: `RequestContext.human(actor)` (the login name when the header is
/// absent) with the request's correlation id. `fresh` is the id drawn for a request without one.
///
/// The correlation is the header's text, as Python stores it: the runtime keeps correlation ids
/// as text, so a header that is not a UUID is kept as it is.
pub fn request_context(
    actor: Option<&[u8]>,
    correlation: Option<&[u8]>,
    login: Option<&str>,
    fresh: wg_types::Id,
) -> Result<wg_runtimespec::RequestContext, WgError> {
    let name = actor_name(actor);
    let correlation = correlation_header(correlation).map_or_else(
        || wg_types::Correlation::from(fresh),
        wg_types::Correlation::new,
    );
    wg_runtimespec::RequestContext::human(name.as_deref(), login, correlation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::{Found, find};

    #[test]
    fn table_order() {
        let table = route_table(&[("GET", "/api/v1/health")], true);
        assert_eq!(
            find(&table, "GET", "/api/v1/health"),
            Found::Route {
                target: &Target::Api(0),
                params: Vec::new()
            }
        );
        assert!(matches!(
            find(&table, "POST", "/mcp/"),
            Found::Route {
                target: Target::Mcp,
                ..
            }
        ));
        assert!(matches!(
            find(&table, "GET", "/pair"),
            Found::Route {
                target: Target::PairForm,
                ..
            }
        ));
        assert!(matches!(
            find(&table, "GET", "/x"),
            Found::Route {
                target: Target::Spa,
                ..
            }
        ));
        assert_eq!(
            find(&table, "DELETE", "/pair"),
            Found::NotAllowed("GET".into())
        );
        let without = route_table(&[], false);
        assert!(matches!(
            find(&without, "GET", "/mcp/"),
            Found::Route {
                target: Target::Spa,
                ..
            }
        ));
    }

    #[test]
    fn openapi_body_is_compact() {
        let body = String::from_utf8(openapi_body().unwrap()).unwrap();
        assert!(body.starts_with("{\"components\":{\"schemas\":{"));
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed, wg_wire::contract::openapi().unwrap());
    }

    #[test]
    fn request_headers() {
        assert_eq!(
            actor_name(Some("x".repeat(150).as_bytes())).map(|n| n.len()),
            Some(100)
        );
        assert_eq!(actor_name(Some(b"")), None);
        assert_eq!(correlation_header(Some("c".repeat(65).as_bytes())), None);
        assert_eq!(
            correlation_header(Some("c".repeat(64).as_bytes())),
            Some("c".repeat(64))
        );
        assert_eq!(correlation_header(Some(b"")), None);
        assert_eq!(
            correlation_header(Some(b"not a uuid")),
            Some("not a uuid".to_owned())
        );
        // UTF-8 bytes are read one character per byte, as Starlette reads them.
        assert_eq!(
            correlation_header(Some("caf\u{e9}".as_bytes())),
            Some("caf\u{c3}\u{a9}".to_owned())
        );
        assert_eq!(header_text(&[0xe9]), "\u{e9}");
    }
}
