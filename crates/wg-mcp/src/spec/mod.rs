//! The pure half of the MCP server (docs/rust-rewrite.md section 4.3, IF-30), as `Pure` modules
//! of `wg-mcp`: the transport rules that are functions of values.
//!
//! * [`accept`]: the `Accept` negotiation of the `/mcp/` endpoint, and the `406` answer to a
//!   refusal (X6);
//! * [`jsonrpc`]: the `400` answer to a POST body that is not a JSON-RPC message (X6);
//! * [`sse`]: the framing of server-sent events;
//! * [`version`]: the `MCP-Protocol-Version` gate, which answers a request naming a revision the
//!   endpoint does not serve before any session check (WG-3819).
//!
//! The descriptors, argument validation, result shapes and permissions that features also use are
//! `wg_iface::mcp`.

pub mod accept;
pub mod jsonrpc;
pub mod sse;
pub mod version;

pub use accept::{
    Gate, Plan, Reject, Reply, SDK_ACCEPT, Step, Touch, Verb, check_body, check_headers,
    error_body, gate, not_acceptable_reply, refusal, reject_status, too_large, touch_up, verb_of,
};
pub use jsonrpc::not_jsonrpc_reply;
pub use sse::frame_events;
pub use version::gate as protocol_version_gate;

/// The server name in `serverInfo`.
pub const SERVER_NAME: &str = "workgraph";

/// The server title in `serverInfo`.
pub const SERVER_TITLE: &str = "Workgraph";

/// The server version in `serverInfo` (`workgraph.__version__`).
pub const SERVER_VERSION: &str = "0.1.0";

/// The `instructions` of the `initialize` result.
pub const INSTRUCTIONS: &str = "Workgraph tracks engineering work across repositories.
Typical agent loop: task_get_ready -> task_claim -> attempt_create -> (work, committing with a
`Task-ID: <KEY>` trailer) -> task_report_progress -> task_link_commit -> task_report_result.
Reporting success moves a task to awaiting verification/review; only humans or configured
verification accept work. Task references accept a UUID or a key such as WG-12.";

/// The protocol version the server speaks (the recorded one).
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// The `initialize` result: protocol version, capabilities (nothing subscribes or changes, and
/// `prompts` is advertised although none exist, as the Python SDK does), server info and
/// instructions.
pub fn initialize_info() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "prompts": {"listChanged": false},
            "resources": {"listChanged": false, "subscribe": false},
            "tools": {"listChanged": false},
        },
        "serverInfo": {
            "name": SERVER_NAME,
            "title": SERVER_TITLE,
            "version": SERVER_VERSION,
        },
        "instructions": INSTRUCTIONS,
    })
}
