//! wg-json: the JSON dialects and timestamp formats.
//!
//! * [`cli_json`] is O01, the CLI `--json` output.
//! * [`http_json`] is O06, the HTTP response body.
//! * [`db_json`] is O10, the JSON text stored in DB columns.
//! * [`log_json`] is O14, one JSON log line.
//!
//! UTC datetimes are `wg_types::Timestamp`, and the formats of a UTC value come from its methods.
//! [`LocalDateTime`] holds the naive and non-UTC datetimes that `Timestamp` cannot represent.
//!
//! The dialects are written by hand rather than as `serde_json` formatters, because `serde_json`
//! cannot emit `NaN` or `Infinity`, cannot hold lone surrogates and has no `ensure_ascii` switch.
//! `serde_json` is only a dev-dependency, used to read the golden vectors.
//!
//! [`to_value`] and [`to_dict`] turn any `#[derive(Serialize)]` struct into a [`Value`] (see the
//! `bridge` module), so a DTO is a struct and the dialects stay here.

mod bridge;
mod json;
mod text;
mod value;

pub use bridge::{BridgeError, dict_field, to_dict, to_value, try_to_value};
pub use json::{cli_json, db_json, http_json, log_json, resource_json};
pub use text::{isoformat, py_float_repr, py_splitlines, when};
pub use value::{Date, DumpError, LocalDateTime, PyStr, Value, dict};
