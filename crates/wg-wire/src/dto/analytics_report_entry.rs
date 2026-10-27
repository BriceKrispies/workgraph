//! One stored report of `GET /reports` (`list_reports`).

use wg_json::{Value, dict};
use wg_types::Timestamp;

use crate::convention::{TimeFormat, text, timestamp};

/// The name of the report dict; it has no OpenAPI component schema.
pub const NAME: &str = "ReportOut";

/// The fields in the order `list_reports` builds them.
pub const FIELDS: [&str; 6] = [
    "id",
    "kind",
    "title",
    "narrative_source",
    "created_at",
    "content",
];

/// A stored report. `created_at` is `isoformat()` of an aware UTC datetime.
pub fn from_parts(
    id: &str,
    kind: &str,
    title: &str,
    narrative_source: &str,
    created_at: Timestamp,
    content: &str,
) -> Value {
    dict!({
        "id": text(id),
        "kind": text(kind),
        "title": text(title),
        "narrative_source": text(narrative_source),
        "created_at": timestamp(created_at, TimeFormat::Isoformat),
        "content": text(content),
    })
}
