//! wg-server: the HTTP API, its security middleware, the SSE helper, the web UI files and the
//! `/mcp/` mount (docs/rust-rewrite.md section 4.8), reproducing `api/app.py` and
//! `api/security.py`.
//!
//! A request goes through [`dispatch`]: the security screen (LAN clients, `Host`, `Origin`, the
//! token), Starlette's routing over the route table, FastAPI's validation of the route's
//! parameters and body, the handler, and the security headers. The rules are
//! `wg-serverspec`'s; this crate holds the sockets, the locks, the file reads and the calls into
//! the runtime.
//!
//! Route groups (sections 9.9, IF-42 to IF-49) live in [`groups`]: each adds its [`ApiRoute`]s
//! and the route-table test checks the implemented groups (`groups::implemented`) against the
//! OpenAPI snapshot.

mod dispatch;
pub mod groups;
mod listen;
mod route;
mod server;
mod sse;
mod ui;

pub use dispatch::router;
pub use listen::{bind_listeners, detect_lan_address, detect_lan_address_with, host_name};
pub use route::{Answer, ApiRequest, ApiRoute, Handler, HandlerFuture, handler};
pub use server::{LanOptions, Server, ServerOptions, serve};
pub use sse::event_stream;
pub use ui::Ui;
pub use wg_serverspec::request::BodySpec;
pub use wg_serverspec::sse::SseEvent;
