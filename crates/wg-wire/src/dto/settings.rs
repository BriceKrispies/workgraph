//! The settings documents of `GET /api/v1/settings` and `PATCH /api/v1/settings`. The config is
//! the dumped effective configuration, passed in as a value.

use wg_json::{PyStr, Value};

use crate::convention::text;

/// The keys of the `GET` document, in Python order.
pub const FIELDS: [&str; 2] = ["config", "paths"];

/// The keys of the `paths` object of the `GET` document.
pub const PATH_FIELDS: [&str; 2] = ["config_file", "data_dir"];

/// The keys of the `PATCH` document.
pub const PATCH_FIELDS: [&str; 1] = ["config"];

/// `GET /api/v1/settings`.
pub fn settings(config: Value, config_file: &str, data_dir: &str) -> Value {
    dict(vec![
        ("config", config),
        (
            "paths",
            dict(vec![
                ("config_file", text(config_file)),
                ("data_dir", text(data_dir)),
            ]),
        ),
    ])
}

/// `PATCH /api/v1/settings`: the configuration after the edit, and nothing else.
pub fn patched(config: Value) -> Value {
    dict(vec![("config", config)])
}

/// A dict value with its keys in the given order.
fn dict(fields: Vec<(&str, Value)>) -> Value {
    Value::Dict(
        fields
            .into_iter()
            .map(|(key, value)| (PyStr::from(key), value))
            .collect(),
    )
}
