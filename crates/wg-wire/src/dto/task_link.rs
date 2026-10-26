//! `TaskLinkOut`.

use wg_json::Value;
use wg_model::entity::task::TaskLink;

use crate::convention::{id, model, optional_id, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "TaskLinkOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 10] = [
    "id",
    "kind",
    "provider",
    "url",
    "normalized_url",
    "repo_full_name",
    "number",
    "repository_id",
    "sources",
    "title",
];

/// A link from the task to something outside Workgraph.
pub fn from_model(link: &TaskLink) -> Value {
    model(
        NAME,
        vec![
            ("id", id(link.id)),
            ("kind", text(&link.kind)),
            ("provider", text(&link.provider)),
            ("url", text(&link.url)),
            ("normalized_url", text(&link.normalized_url)),
            ("repo_full_name", text(&link.repo_full_name)),
            (
                "number",
                link.number
                    .map_or(Value::Null, |number| Value::Int(i128::from(number))),
            ),
            ("repository_id", optional_id(link.repository_id)),
            (
                "sources",
                Value::List(link.sources.iter().map(|source| text(source)).collect()),
            ),
            ("title", text(&link.title)),
        ],
    )
}
