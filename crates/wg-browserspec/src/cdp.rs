//! The DevTools Protocol messages a capture sends, and the parsing of what comes back.

use base64::Engine;
use serde_json::{Value, json};
use wg_types::WgError;
use wg_validate::pydantic::py_repr;

use crate::options::CaptureOptions;

/// One DevTools method call: its name and parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct CdpCall {
    pub method: &'static str,
    pub params: Value,
}

/// A reply to one request: its result, or the browser's error message.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Result(Value),
    Error(String),
}

/// The calls that set up the viewport and load the page, in the order they are sent.
#[must_use]
pub fn capture_calls(options: &CaptureOptions, url: &str) -> Vec<CdpCall> {
    let mut calls = vec![CdpCall {
        method: "Emulation.setDeviceMetricsOverride",
        params: json!({
            "width": options.width,
            "height": options.height,
            "deviceScaleFactor": options.scale,
            "mobile": options.mobile,
        }),
    }];
    if options.mobile {
        calls.push(CdpCall {
            method: "Emulation.setTouchEmulationEnabled",
            params: json!({"enabled": true, "maxTouchPoints": 5}),
        });
    }
    if let Some(theme) = &options.theme {
        calls.push(CdpCall {
            method: "Emulation.setEmulatedMedia",
            params: json!({"features": [{"name": "prefers-color-scheme", "value": theme}]}),
        });
    }
    calls.push(CdpCall {
        method: "Page.navigate",
        params: json!({"url": url}),
    });
    calls
}

/// The call that takes the PNG once the page is ready.
#[must_use]
pub fn capture_screenshot_call() -> CdpCall {
    CdpCall {
        method: "Page.captureScreenshot",
        params: json!({"format": "png"}),
    }
}

/// The call that evaluates `expression` in the page and returns it by value.
#[must_use]
pub fn evaluate_call(expression: &str) -> CdpCall {
    CdpCall {
        method: "Runtime.evaluate",
        params: json!({"expression": expression, "returnByValue": true}),
    }
}

/// The JSON text of a request with `id`.
#[must_use]
pub fn request_text(id: u64, call: &CdpCall) -> String {
    json!({"id": id, "method": call.method, "params": call.params}).to_string()
}

/// The reply to request `id` in `text`, or `None` when the text is not JSON, is an event, or
/// answers another request.
#[must_use]
pub fn parse_reply(text: &str, id: u64) -> Option<Reply> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value.get("id").and_then(Value::as_u64) != Some(id) {
        return None;
    }
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Some(Reply::Error(message.to_owned()));
    }
    Some(Reply::Result(
        value.get("result").cloned().unwrap_or_else(|| json!({})),
    ))
}

/// The value of an evaluation, from the `result` of a `Runtime.evaluate` reply.
#[must_use]
pub fn evaluated_true(result: &Value) -> bool {
    result.pointer("/result/value").and_then(Value::as_bool) == Some(true)
}

/// The expression that is true once the page has loaded and the selectors have settled.
#[must_use]
pub fn readiness_expression(wait_for: Option<&str>, wait_gone: Option<&str>) -> String {
    let mut ready = "document.readyState === 'complete'".to_owned();
    if let Some(selector) = wait_for {
        ready.push_str(&format!(
            " && document.querySelector({}) !== null",
            json_string(selector)
        ));
    }
    if let Some(selector) = wait_gone {
        ready.push_str(&format!(
            " && document.querySelector({}) === null",
            json_string(selector)
        ));
    }
    ready
}

/// The error when the page is not ready within `timeout` seconds.
#[must_use]
pub fn wait_timeout_error(
    timeout: f64,
    url: &str,
    wait_for: Option<&str>,
    wait_gone: Option<&str>,
) -> WgError {
    // Python prints each selector with `repr(str)`, as `{wait_for!r}` in `integrations/browser.py`.
    let quoted = |selector: &str| py_repr(&Value::String(selector.to_owned()));
    let mut waits = vec!["page load".to_owned()];
    if let Some(selector) = wait_for {
        waits.push(format!("{} to appear", quoted(selector)));
    }
    if let Some(selector) = wait_gone {
        waits.push(format!("{} to disappear", quoted(selector)));
    }
    WgError::provider_unavailable(format!(
        "timed out after {timeout:.0}s waiting for {} at {url}",
        waits.join(" and ")
    ))
}

/// The DevTools WebSocket of the first page target in a `/json/list` body.
#[must_use]
pub fn page_websocket_url(list_json: &str) -> Option<String> {
    let targets: Value = serde_json::from_str(list_json).ok()?;
    targets.as_array()?.iter().find_map(|target| {
        if target.get("type")?.as_str()? != "page" {
            return None;
        }
        let url = target.get("webSocketDebuggerUrl")?.as_str()?;
        (!url.is_empty()).then(|| url.to_owned())
    })
}

/// The port named by the first line of Chrome's `DevToolsActivePort` file.
#[must_use]
pub fn devtools_port(file_text: &str) -> Option<u16> {
    file_text.lines().next()?.trim().parse().ok()
}

/// The PNG bytes in the `result` of a `Page.captureScreenshot` reply.
///
/// # Errors
/// [`ErrorCode::ProviderUnavailable`] when the reply has no valid base64 `data`.
pub fn screenshot_bytes(result: &Value) -> Result<Vec<u8>, WgError> {
    let data = result
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| WgError::provider_unavailable("browser returned no screenshot data"))?;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|err| {
            WgError::provider_unavailable(format!(
                "browser returned invalid screenshot data: {err}"
            ))
        })
}

fn json_string(text: &str) -> String {
    // Serialising a string cannot fail.
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wg_types::ErrorCode;

    #[test]
    fn dark_touch_phone_emulates_media_and_touch_in_order() {
        let options = CaptureOptions {
            width: 390,
            height: 844,
            scale: 3.0,
            theme: Some("dark".to_owned()),
            mobile: true,
            ..CaptureOptions::default()
        };
        let methods: Vec<&str> = capture_calls(&options, "http://x/")
            .iter()
            .map(|call| call.method)
            .collect();
        assert_eq!(
            methods,
            [
                "Emulation.setDeviceMetricsOverride",
                "Emulation.setTouchEmulationEnabled",
                "Emulation.setEmulatedMedia",
                "Page.navigate",
            ]
        );
        let calls = capture_calls(&options, "http://x/");
        assert_eq!(calls[0].params["mobile"], json!(true));
        assert_eq!(
            calls[2].params["features"],
            json!([{"name": "prefers-color-scheme", "value": "dark"}])
        );
    }

    #[test]
    fn desktop_default_leaves_emulation_alone() {
        let calls = capture_calls(&CaptureOptions::default(), "http://x/");
        let methods: Vec<&str> = calls.iter().map(|call| call.method).collect();
        assert_eq!(
            methods,
            ["Emulation.setDeviceMetricsOverride", "Page.navigate"]
        );
        assert_eq!(calls[0].params["mobile"], json!(false));
    }

    #[test]
    fn request_text_carries_id_method_and_params() {
        let text = request_text(7, &capture_screenshot_call());
        let value: Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["id"], json!(7));
        assert_eq!(value["method"], json!("Page.captureScreenshot"));
        assert_eq!(value["params"]["format"], json!("png"));
    }

    #[test]
    fn replies_are_matched_by_id_and_errors_surface() {
        let ok = r#"{"id":3,"result":{"data":"AAAA"}}"#;
        assert_eq!(
            parse_reply(ok, 3),
            Some(Reply::Result(json!({"data": "AAAA"})))
        );
        assert_eq!(parse_reply(ok, 4), None);
        assert_eq!(parse_reply(r#"{"method":"Page.loadEventFired"}"#, 3), None);
        let refused = r#"{"id":3,"error":{"message":"nope"}}"#;
        assert_eq!(
            parse_reply(refused, 3),
            Some(Reply::Error("nope".to_owned()))
        );
        assert_eq!(parse_reply("not json", 3), None);
    }

    #[test]
    fn readiness_checks_the_selectors_in_order() {
        assert_eq!(
            readiness_expression(None, Some("[aria-busy=\"true\"]")),
            "document.readyState === 'complete' && document.querySelector(\"[aria-busy=\\\"true\\\"]\") === null"
        );
        assert_eq!(
            readiness_expression(Some("main"), None),
            "document.readyState === 'complete' && document.querySelector(\"main\") !== null"
        );
    }

    #[test]
    fn evaluated_true_reads_the_boolean_value() {
        assert!(evaluated_true(
            &json!({"result": {"type": "boolean", "value": true}})
        ));
        assert!(!evaluated_true(
            &json!({"result": {"type": "boolean", "value": false}})
        ));
        assert!(!evaluated_true(&json!({})));
    }

    #[test]
    fn timeout_message_names_every_wait() {
        let err = wait_timeout_error(5.4, "http://x/", Some("main"), Some("[aria-busy=\"true\"]"));
        assert_eq!(err.code, ErrorCode::ProviderUnavailable);
        assert_eq!(
            err.message,
            "timed out after 5s waiting for page load and 'main' to appear and '[aria-busy=\"true\"]' to disappear at http://x/"
        );
    }

    #[test]
    fn timeout_message_quotes_selectors_like_python_repr() {
        let err = wait_timeout_error(3.0, "http://x/", None, Some("a[title='x']"));
        assert_eq!(
            err.message,
            "timed out after 3s waiting for page load and \"a[title='x']\" to disappear at http://x/"
        );
    }

    #[test]
    fn page_target_is_the_first_page_with_a_socket() {
        let list = r#"[
            {"type":"service_worker","webSocketDebuggerUrl":"ws://a"},
            {"type":"page","webSocketDebuggerUrl":""},
            {"type":"page","webSocketDebuggerUrl":"ws://127.0.0.1:9/devtools/page/1"}
        ]"#;
        assert_eq!(
            page_websocket_url(list).as_deref(),
            Some("ws://127.0.0.1:9/devtools/page/1")
        );
        assert_eq!(page_websocket_url("[]"), None);
        assert_eq!(page_websocket_url("oops"), None);
    }

    #[test]
    fn devtools_port_reads_the_first_line() {
        assert_eq!(devtools_port("51234\n/devtools/browser/abc\n"), Some(51234));
        assert_eq!(devtools_port(""), None);
        assert_eq!(devtools_port("x"), None);
    }

    #[test]
    fn screenshot_bytes_decode_the_png_data() {
        let bytes = screenshot_bytes(&json!({"data": "iVBORw0KGgo="})).expect("decodes");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert!(screenshot_bytes(&json!({})).is_err());
        assert!(screenshot_bytes(&json!({"data": "!!"})).is_err());
    }
}
