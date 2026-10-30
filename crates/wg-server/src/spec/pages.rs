//! The fixed pages outside the API (X2, X3, X7): FastAPI's Swagger UI page and its OAuth2
//! redirect helper, byte for byte as recorded (they load swagger-ui from the jsDelivr CDN, as
//! FastAPI's do), and the placeholder shown when the web UI has not been built.

/// `GET /api/docs`: `get_swagger_ui_html(openapi_url="/api/openapi.json", title="Workgraph -
/// Swagger UI", oauth2_redirect_url="/docs/oauth2-redirect")`.
pub const SWAGGER_HTML: &str = include_str!("../pages/swagger.html");

/// `GET /docs/oauth2-redirect`: `get_swagger_ui_oauth2_redirect_html()`.
pub const OAUTH2_REDIRECT_HTML: &str = include_str!("../pages/oauth2-redirect.html");

/// The page `/` serves when `index.html` is missing.
pub const UI_NOT_BUILT_HTML: &str = "<!doctype html><html><head><meta charset='utf-8'><title>Workgraph</title></head><body>\
<h1>Workgraph API is running</h1><p>The web UI has not been built. Run \
<code>bun install --cwd web</code> and <code>bun run --cwd web build</code>, then reload.</p>\
<p>API: <code>/api/v1</code></p></body></html>";

/// `_index_html(token)`: the built `index.html` with the token meta tag inserted before the first
/// `</head>`, or the placeholder when there is no `index.html`.
pub fn index_html(index: Option<&str>, token: &str) -> String {
    match index {
        None => UI_NOT_BUILT_HTML.to_owned(),
        Some(html) => html.replacen(
            "</head>",
            &format!("<meta name=\"workgraph-token\" content=\"{token}\"></head>"),
            1,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_meta_goes_before_the_first_head_close() {
        assert_eq!(
            index_html(Some("<head></head><x></head>"), "t"),
            "<head><meta name=\"workgraph-token\" content=\"t\"></head><x></head>"
        );
        assert_eq!(index_html(None, "t"), UI_NOT_BUILT_HTML);
        assert!(SWAGGER_HTML.contains("Workgraph - Swagger UI"));
        assert!(OAUTH2_REDIRECT_HTML.contains("Swagger UI: OAuth2 Redirect"));
    }
}
