//! `workgraph ui screenshot` (`cli/commands/ui.py`): the URLs, the page-ready selector and the
//! result the command prints.

use clap::ArgMatches;
use wg_json::{PyStr, Value, py_float_repr};

use crate::CliFailure;
use crate::task::option_text;

/// The numeric option `key`: the typed text, else `default`; a bad number is a usage failure.
///
/// # Errors
/// A usage failure (exit 2) for text that is not a number.
pub fn number<T: std::str::FromStr>(
    m: &ArgMatches,
    key: &str,
    default: T,
) -> Result<T, CliFailure> {
    match option_text(m, key) {
        None => Ok(default),
        Some(text) => text.parse().map_err(|_| {
            CliFailure::fail(
                format!("Invalid value for '{key}': '{text}' is not a valid number"),
                2,
            )
        }),
    }
}

/// The base URL with a single trailing slash: `--url`, else the configured `host:port`.
#[must_use]
pub fn base_url(url: Option<&str>, host: &str, port: u16) -> String {
    match url.filter(|text| !text.is_empty()) {
        Some(text) => format!("{}/", text.trim_end_matches('/')),
        None => format!("http://{host}:{port}/"),
    }
}

/// `urljoin(base, "api/v1/health")` for a base ending in a slash.
#[must_use]
pub fn health_url(base: &str) -> String {
    format!("{base}api/v1/health")
}

/// `urljoin(base, page.lstrip("/"))` for a base ending in a slash.
#[must_use]
pub fn target_url(base: &str, page: &str) -> String {
    format!("{base}{}", page.trim_start_matches('/'))
}

/// The refusal when nothing answers the health endpoint.
#[must_use]
pub fn no_server_message(base: &str, reason: &str) -> String {
    format!("no Workgraph server at {base} ({reason}); start one with `workgraph serve`")
}

/// The selector to wait for: `--wait-for`, else the page's own ready marker, else the app shell.
#[must_use]
pub fn wait_selector(page: &str, wait_for: Option<&str>) -> String {
    let route = page
        .trim_start_matches('/')
        .split('?')
        .next()
        .and_then(|path| path.split('/').next())
        .unwrap_or_default();
    wait_for.map_or_else(
        || {
            if route == "graph" {
                "[data-testid=\"graph-counts\"]".to_owned()
            } else {
                "#root > *".to_owned()
            }
        },
        str::to_owned,
    )
}

/// What the command reports about a capture.
#[derive(Debug, Clone, PartialEq)]
pub struct Shot {
    pub url: String,
    pub file: String,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub browser: String,
    pub theme: Option<String>,
    pub mobile: bool,
}

impl Shot {
    /// The JSON result, keys in Python's order.
    #[must_use]
    pub fn value(&self) -> Value {
        let text = |text: &str| Value::Str(PyStr::from(text));
        let entries = [
            ("url", text(&self.url)),
            ("file", text(&self.file)),
            ("width", Value::Int(i128::from(self.width))),
            ("height", Value::Int(i128::from(self.height))),
            ("scale", Value::Float(self.scale)),
            ("browser", text(&self.browser)),
            ("theme", self.theme.as_deref().map_or(Value::Null, text)),
            ("mobile", Value::Bool(self.mobile)),
        ];
        Value::Dict(
            entries
                .into_iter()
                .map(|(key, value)| (PyStr::from(key), value))
                .collect(),
        )
    }

    /// The `kv` pairs of the human output.
    #[must_use]
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("url", self.url.clone()),
            ("file", self.file.clone()),
            ("width", self.width.to_string()),
            ("height", self.height.to_string()),
            ("scale", py_float_repr(self.scale)),
            ("browser", self.browser.clone()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_selectors() {
        assert_eq!(base_url(Some("http://h:1//"), "x", 2), "http://h:1/");
        assert_eq!(base_url(None, "127.0.0.1", 8765), "http://127.0.0.1:8765/");
        assert_eq!(
            target_url("http://h/", "/tasks/WG-1"),
            "http://h/tasks/WG-1"
        );
        assert_eq!(
            wait_selector("graph?repo=x", None),
            "[data-testid=\"graph-counts\"]"
        );
        assert_eq!(wait_selector("/tasks/WG-1", None), "#root > *");
        assert_eq!(wait_selector("graph", Some("#a")), "#a");
    }
}
