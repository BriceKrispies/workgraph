//! The result of `POST /reports/retrospective` (`generate_retrospective`).

use wg_json::Value;

use crate::wire_dto;

wire_dto! {
    /// A generated retrospective. `facts` is the measured block the narrative was written from;
    /// its own shape is built by the analytics engine and passed in as a value.
    pub struct RetrospectiveOut<'a>("RetrospectiveOut")
    dict from_parts(id: &'a str, title: &'a str, narrative_source: &'a str, content: &'a str, facts: Value) {
        id: &'a str = id,
        title: &'a str = title,
        narrative_source: &'a str = narrative_source,
        content: &'a str = content,
        facts: Value = facts,
    }
}
