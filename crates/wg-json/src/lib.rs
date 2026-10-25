//! wg-json: the three JSON dialects and timestamp formats.
//!
//! * [`cli_json`] is O01, the CLI `--json` output.
//! * [`http_json`] is O06, the HTTP response body.
//! * [`db_json`] is O10, the JSON text stored in DB columns.
//!
//! The dialects are written by hand rather than as `serde_json` formatters, because `serde_json`
//! cannot emit `NaN` or `Infinity`, cannot hold lone surrogates and has no `ensure_ascii` switch.
//! `serde_json` is only a dev-dependency, used to read the golden vectors.

mod json;
mod text;
mod value;

pub use json::{cli_json, db_json, http_json};
pub use text::{isoformat, py_float_repr, when};
pub use value::{Date, DateTime, DumpError, PyStr, Value};
