//! wg-browserspec: the pure half of `wg-browser`.
//!
//! The browser search order, the capture options and their validation, the DevTools messages and
//! the parsing of their replies, and the launch arguments. Nothing here starts a process, opens a
//! socket, reads the environment or the clock: the shell passes those values in.

mod cdp;
mod error;
mod locate;
mod options;

pub use cdp::{
    CdpCall, Reply, capture_calls, capture_screenshot_call, devtools_port, evaluate_call,
    evaluated_true, page_websocket_url, parse_reply, readiness_expression, request_text,
    screenshot_bytes, wait_timeout_error,
};
pub use error::{io_failure, unavailable};
pub use locate::{
    BROWSER_ENV, Platform, SEARCH_NAMES, browser_not_found, explicit_candidates, install_locations,
    locate_browser, no_browser_installed, path_candidates, requested_browser, search_candidates,
};
pub use options::{CaptureOptions, launch_args};
