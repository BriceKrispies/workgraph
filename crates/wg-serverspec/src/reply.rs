//! [`Reply`]: a complete HTTP response as a value, and the Starlette/FastAPI responses the server
//! sends (`JSONResponse`, `HTMLResponse`, `RedirectResponse`, FastAPI's `{"detail": ...}`
//! errors), with the security headers of O07.

use serde_json::Value as Json;
use wg_json::{Value, dict, http_json};
use wg_types::WgError;
use wg_wire::convention::text;
use wg_wire::error::{error_envelope, validation_envelope};

/// `application/json`, as Starlette's `JSONResponse` sends it (no charset).
pub const JSON_TYPE: &str = "application/json";
/// `text/html; charset=utf-8`, as Starlette's `HTMLResponse` sends it.
pub const HTML_TYPE: &str = "text/html; charset=utf-8";

/// An HTTP response: status, headers in order (names in lower case), body bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    /// A response with a body and a `content-type`.
    pub fn new(status: u16, content_type: &str, body: Vec<u8>) -> Reply {
        Reply {
            status,
            headers: vec![("content-type".to_owned(), content_type.to_owned())],
            body,
        }
    }

    /// `JSONResponse(value, status_code=status)`: compact, `ensure_ascii=False`,
    /// `allow_nan=False`. A value JSON cannot hold (NaN) is Starlette's 500.
    pub fn json(status: u16, value: &Value) -> Reply {
        match http_json(value) {
            Ok(body) => Reply::new(status, JSON_TYPE, body.into_bytes()),
            Err(_) => internal_error(),
        }
    }

    /// `HTMLResponse(text, status_code=status)`.
    pub fn html(status: u16, body: &str) -> Reply {
        Reply::new(status, HTML_TYPE, body.as_bytes().to_vec())
    }

    /// `RedirectResponse(location, status_code=303)`: no body and no content type.
    pub fn see_other(location: &str) -> Reply {
        Reply {
            status: 303,
            headers: vec![("location".to_owned(), location.to_owned())],
            body: Vec::new(),
        }
    }

    /// The reply with one more header.
    pub fn with_header(self, name: &str, value: &str) -> Reply {
        let mut headers = self.headers;
        headers.push((name.to_ascii_lowercase(), value.to_owned()));
        Reply { headers, ..self }
    }

    /// The first value of a header, by lower-case name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The reply with each `(name, value)` added unless a header of that name is present
    /// (`MutableHeaders.setdefault`).
    pub fn with_defaults(self, defaults: &[(&str, &str)]) -> Reply {
        let mut headers = self.headers;
        for (name, value) in defaults {
            if !headers.iter().any(|(key, _)| key == name) {
                headers.push(((*name).to_owned(), (*value).to_owned()));
            }
        }
        Reply { headers, ..self }
    }

    /// The body without the payload, for a `HEAD` request: status and headers stay.
    pub fn without_body(self) -> Reply {
        Reply {
            body: Vec::new(),
            ..self
        }
    }
}

/// `{"error": {"code": code, "message": message}}`: the envelope the security middleware and the
/// SPA's 404 build by hand, without `details`.
pub fn bare_error(status: u16, code: &str, message: &str) -> Reply {
    let body = dict!({
        "error": dict!({
            "code": text(code),
            "message": text(message),
        }),
    });
    Reply::json(status, &body)
}

/// A domain error: the full envelope with the status of its code (the `WorkgraphError` handler).
pub fn error_reply(error: &WgError) -> Reply {
    Reply::json(error.http_status(), &error_envelope(error))
}

/// The 422 of a request that failed validation, with the Pydantic error items.
pub fn validation_reply(errors: &[Json]) -> Reply {
    Reply::json(422, &validation_envelope(errors))
}

/// FastAPI's `HTTPException` body: `{"detail": text}`.
pub fn detail(status: u16, message: &str) -> Reply {
    Reply::json(status, &dict!({ "detail": text(message) }))
}

/// Starlette's 405 for a path that matched with another method: `Allow` lists the methods of the
/// first route that matched the path.
pub fn method_not_allowed(allow: &str) -> Reply {
    detail(405, "Method Not Allowed").with_header("allow", allow)
}

/// FastAPI's 400 when the body could not be read as text (`There was an error parsing the body`).
pub fn body_parse_error() -> Reply {
    detail(400, "There was an error parsing the body")
}

/// Starlette's `ServerErrorMiddleware` answer to an unhandled failure.
pub fn internal_error() -> Reply {
    Reply::new(
        500,
        "text/plain; charset=utf-8",
        b"Internal Server Error".to_vec(),
    )
}

/// The `security_headers` middleware (O07): `nosniff`, `DENY`, `no-referrer` on every response
/// the application produces, and `Cache-Control: no-store` under `/api/`. The security
/// middleware's own rejections (401, 403, 421) do not pass through it and carry none of them.
pub fn security_defaults(path: &str) -> Vec<(&'static str, &'static str)> {
    let mut defaults = vec![
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
    ];
    if path.starts_with("/api/") {
        defaults.push(("cache-control", "no-store"));
    }
    defaults
}

/// `Reply::with_defaults(security_defaults(path))`.
pub fn secured(reply: Reply, path: &str) -> Reply {
    reply.with_defaults(&security_defaults(path))
}

/// A JSON object from `serde_json` for a JSON reply, keeping key order.
pub fn json_reply(status: u16, value: &Json) -> Reply {
    Reply::json(status, &wg_wire::convention::json_value(value))
}

/// The reply as an `http` response; `head` drops the body (a `HEAD` request). A header that is
/// not valid HTTP is left out.
pub fn to_http(reply: Reply, head: bool) -> http::Response<Vec<u8>> {
    let status =
        http::StatusCode::from_u16(reply.status).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    let builder = reply
        .headers
        .iter()
        .filter_map(|(name, value)| {
            Some((
                http::HeaderName::from_bytes(name.as_bytes()).ok()?,
                http::HeaderValue::from_str(value).ok()?,
            ))
        })
        .fold(
            http::Response::builder().status(status),
            |builder, (name, value)| builder.header(name, value),
        );
    let body = if head { Vec::new() } else { reply.body };
    builder
        .body(body)
        .unwrap_or_else(|_| http::Response::new(Vec::new()))
}

/// A response's headers with the security headers added where absent (for responses built
/// outside [`Reply`]: event streams and the MCP transport).
pub fn secured_headers(headers: http::HeaderMap, path: &str) -> http::HeaderMap {
    let mut headers = headers;
    for (name, value) in security_defaults(path) {
        if !headers.contains_key(name) {
            headers.insert(name, http::HeaderValue::from_static(value));
        }
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_error_has_no_details() {
        let reply = bare_error(401, "unauthorized", "missing or invalid API token");
        assert_eq!(
            String::from_utf8(reply.body).unwrap(),
            r#"{"error":{"code":"unauthorized","message":"missing or invalid API token"}}"#
        );
        assert_eq!(
            reply.headers,
            vec![("content-type".into(), "application/json".into())]
        );
    }

    #[test]
    fn security_defaults_do_not_override() {
        let reply = secured(
            Reply::html(200, "x").with_header("cache-control", "max-age=1"),
            "/api/x",
        );
        assert_eq!(reply.header("cache-control"), Some("max-age=1"));
        assert_eq!(reply.header("x-frame-options"), Some("DENY"));
        let page = secured(Reply::html(200, "x"), "/assets/a.js");
        assert_eq!(page.header("cache-control"), None);
    }

    #[test]
    fn method_not_allowed_body() {
        let reply = method_not_allowed("GET");
        assert_eq!(reply.body, br#"{"detail":"Method Not Allowed"}"#.to_vec());
        assert_eq!(reply.header("allow"), Some("GET"));
    }
}
