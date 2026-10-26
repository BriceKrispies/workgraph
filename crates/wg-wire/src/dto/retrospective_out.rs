//! The result of `POST /reports/retrospective` (`generate_retrospective`).

use wg_json::Value;

use crate::convention::{dict, text};

/// The name of the result dict; it has no OpenAPI component schema.
pub const NAME: &str = "RetrospectiveOut";

/// The fields in the order `generate_retrospective` returns them.
pub const FIELDS: [&str; 5] = ["id", "title", "narrative_source", "content", "facts"];

/// A generated retrospective. `facts` is the measured block the narrative was written from; its
/// own shape is built by the analytics engine and passed in as a value.
pub fn from_parts(
    id: &str,
    title: &str,
    narrative_source: &str,
    content: &str,
    facts: Value,
) -> Value {
    dict(vec![
        ("id", text(id)),
        ("title", text(title)),
        ("narrative_source", text(narrative_source)),
        ("content", text(content)),
        ("facts", facts),
    ])
}
