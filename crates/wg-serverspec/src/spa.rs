//! The single-page app route (X7, `GET /{path:path}`): API and MCP paths get the JSON 404, a file
//! of the built UI is served as it is, anything else gets `index.html` with the token (or a
//! redirect to `/pair` for an unpaired LAN browser).
//!
//! The shell does the file reads; these functions decide what to read and what to answer.

use crate::reply::{Reply, bare_error};
use crate::security::{Incoming, Policy, trusted_browser};

/// The first step of the SPA route for the path (without its leading `/`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaStep {
    /// `api/...` and `mcp...` paths: the JSON 404 envelope.
    NotFound(Reply),
    /// Serve this file of the UI directory if it is one, else fall through to the page.
    File(String),
    /// Serve the page.
    Page,
}

/// `spa(path)`: what to try first.
pub fn spa_step(path: &str) -> SpaStep {
    if path.starts_with("api/") || path.starts_with("mcp") {
        return SpaStep::NotFound(bare_error(404, "not_found", &format!("/{path} not found")));
    }
    match static_path(path) {
        Some(relative) => SpaStep::File(relative),
        None => SpaStep::Page,
    }
}

/// The file a non-empty path names inside the UI directory, as `/`-separated segments with `.`
/// and `..` resolved: `None` when the path is empty, leaves the directory, or names a drive or
/// stream (`(assets / path).resolve()` must stay under `assets`). Backslashes separate too, as
/// on Windows.
pub fn static_path(path: &str) -> Option<String> {
    if path.is_empty() || path.contains(':') || path.contains('\0') {
        return None;
    }
    let segments =
        path.split(['/', '\\'])
            .try_fold(Vec::new(), |mut kept: Vec<&str>, segment| {
                match segment {
                    "" | "." => {}
                    ".." => {
                        kept.pop()?;
                    }
                    other => kept.push(other),
                }
                Some(kept)
            })?;
    (!segments.is_empty()).then(|| segments.join("/"))
}

/// The page: `index.html` with the token for a trusted browser, else a redirect to `/pair`.
pub fn spa_page(policy: &Policy, req: &Incoming, index: Option<&str>) -> Reply {
    if !trusted_browser(policy, req) {
        return Reply::see_other("/pair");
    }
    Reply::html(200, &crate::pages::index_html(index, &policy.token))
        .with_header("cache-control", "no-store")
}

/// `mimetypes.guess_type(name)[0] or "text/plain"`, as the recorded Python reports it on Windows,
/// for the files a Vite build writes.
pub fn guess_type(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "js" => "application/javascript",
        "mjs" => "text/javascript",
        "css" => "text/css",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webmanifest" => "application/manifest+json",
        "wasm" => "application/wasm",
        "xml" => "text/xml",
        _ => "text/plain",
    }
}

/// `FileResponse(path)`'s content type: the guessed type, with `; charset=utf-8` for `text/*`.
pub fn file_content_type(name: &str) -> String {
    let media = guess_type(name);
    if media.starts_with("text/") {
        format!("{media}; charset=utf-8")
    } else {
        media.to_owned()
    }
}

/// A file of the UI, as `FileResponse` sends it (no cache header).
pub fn file_reply(name: &str, bytes: Vec<u8>) -> Reply {
    Reply::new(200, &file_content_type(name), bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps() {
        assert!(matches!(spa_step("api/v1/nope"), SpaStep::NotFound(r) if r.status == 404));
        assert!(matches!(spa_step("mcpx"), SpaStep::NotFound(_)));
        assert_eq!(spa_step(""), SpaStep::Page);
        assert_eq!(
            spa_step("assets/app.js"),
            SpaStep::File("assets/app.js".into())
        );
        assert_eq!(spa_step("tasks/WG-1"), SpaStep::File("tasks/WG-1".into()));
    }

    #[test]
    fn static_paths_stay_inside() {
        assert_eq!(
            static_path("assets/../index.html").as_deref(),
            Some("index.html")
        );
        assert_eq!(static_path("a/./b//c").as_deref(), Some("a/b/c"));
        assert_eq!(static_path("../secret"), None);
        assert_eq!(static_path("a\\..\\..\\x"), None);
        assert_eq!(static_path("C:/Windows/win.ini"), None);
        assert_eq!(static_path("assets/.."), None);
    }

    #[test]
    fn content_types() {
        assert_eq!(file_content_type("assets/app.js"), "application/javascript");
        assert_eq!(
            file_content_type("assets/app.css"),
            "text/css; charset=utf-8"
        );
        assert_eq!(file_content_type("x.woff2"), "text/plain; charset=utf-8");
        assert_eq!(file_content_type("x.map"), "text/plain; charset=utf-8");
        assert_eq!(file_content_type("favicon.ICO"), "image/x-icon");
    }
}
