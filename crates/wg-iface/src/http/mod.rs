//! The HTTP rules the features and the server share (docs/rust-rewrite.md section 4.8): the
//! operation tables and their groups, FastAPI's parameter and body validation with its 422 and
//! 400 bodies (including Python's JSON decode errors), responses as values and the security
//! headers (O07), `ipaddress`, `urllib.parse` and `repr`/`html.escape`.

pub mod form;
pub mod groups;
pub mod lan;
pub mod net;
pub mod operations;
pub use wg_validate::pyjson;
pub mod reply;
pub mod request;
pub mod serve;
pub mod text;
