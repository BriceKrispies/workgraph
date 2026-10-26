//! `TaskFileOut`.

use wg_json::Value;
use wg_model::entity::task::TaskFile;

use crate::convention::{model, optional_text, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "TaskFileOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 3] = ["path", "anchor", "note"];

/// One file the task expects to change.
pub fn from_model(file: &TaskFile) -> Value {
    model(
        NAME,
        vec![
            ("path", text(&file.path)),
            ("anchor", optional_text(file.anchor.as_deref())),
            ("note", text(&file.note)),
        ],
    )
}
