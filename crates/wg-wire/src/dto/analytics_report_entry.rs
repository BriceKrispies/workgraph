//! One stored report of `GET /reports` (`list_reports`).

use wg_types::Timestamp;

use crate::convention::isoformat;
use crate::wire_dto;

wire_dto! {
    /// A stored report, fields in the order `list_reports` builds them. `created_at` is
    /// `isoformat()` of an aware UTC datetime.
    pub struct ReportOut<'a>("ReportOut")
    dict from_parts(id: &'a str, kind: &'a str, title: &'a str, narrative_source: &'a str, created_at: Timestamp, content: &'a str) {
        id: &'a str = id,
        kind: &'a str = kind,
        title: &'a str = title,
        narrative_source: &'a str = narrative_source,
        #[serde(serialize_with = "isoformat")]
        created_at: Timestamp = created_at,
        content: &'a str = content,
    }
}
