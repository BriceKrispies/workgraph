//! The pure parts of the classification and embedding writes: the ids a decision uses, the event
//! drafts, the run identity of a classification attempt, the payloads of the jobs it queues and
//! the errors it raises. The shell loads the rows and performs the writes; these build the values.

use serde_json::{Map, Value as Json};
use wg_intel::RunRow;
use wg_model::entity::JsonObject;
use wg_model::{EventDraft, EventPayload, IdSeq, Stamp};
use wg_types::{ErrorCode, Id, WgError};

/// The first `count` ids of a decision's stamp, in order.
#[must_use]
pub fn ids_of(stamp: &Stamp, count: usize) -> Vec<Id> {
    take_ids(stamp.ids(), count)
}

/// The first `count` ids of a sequence.
fn take_ids(sequence: IdSeq, count: usize) -> Vec<Id> {
    (0..count)
        .fold((Vec::new(), sequence), |(ids, seq), _| {
            let (id, next) = seq.next();
            (ids.into_iter().chain([id]).collect(), next)
        })
        .0
}

/// An event of the decision: stamped with the decision's actor and clock, on `entity` in
/// `workspace`.
#[must_use]
pub fn event(
    stamp: &Stamp,
    id: Id,
    workspace: Id,
    entity: Id,
    payload: EventPayload,
) -> EventDraft {
    EventDraft::new(stamp, id, Some(workspace), entity, payload)
}

/// The attempt a classification run makes on one task, with the identity of its row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub task_id: Id,
    pub provider: String,
    pub model: String,
    pub digest: String,
    pub batch_id: Option<Id>,
}

impl Attempt {
    /// The run row of this attempt, with the row id, the actor and the time of the decision.
    #[must_use]
    pub fn run_row(&self, id: Id, created_by: String, created_at: wg_types::Timestamp) -> RunRow {
        RunRow {
            id,
            task_id: self.task_id,
            provider: self.provider.clone(),
            model: self.model.clone(),
            input_hash: self.digest.clone(),
            batch_id: self.batch_id,
            created_by,
            created_at,
        }
    }
}

/// The validation status a failed row carries. Anything but the known statuses is an `error`.
#[must_use]
pub fn validation_status(validation: &str) -> &'static str {
    match validation {
        "provider_unavailable" => "provider_unavailable",
        "invalid_output" => "invalid_output",
        _ => "error",
    }
}

/// A job payload from its entries (`{"task_id": ..., ...}`).
#[must_use]
pub fn job_payload(entries: Vec<(&str, Json)>) -> JsonObject {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

/// A string as a JSON value, or null.
#[must_use]
pub fn text_or_null(value: &Option<String>) -> Json {
    value.clone().map_or(Json::Null, Json::String)
}

/// The text `f"{value}"` gives an optional value in Python: the value, or `None`.
#[must_use]
pub fn text_or_none(value: &Option<String>) -> String {
    value.clone().unwrap_or_else(|| "None".to_owned())
}

/// The text of an optional string, or `None` (`str(value or None)` in a payload).
#[must_use]
pub fn json_text(value: Option<&str>) -> Json {
    value.map_or(Json::Null, |text| Json::String(text.to_owned()))
}

/// The prompt of a classification: the task text and its tags.
#[must_use]
pub fn prompt_of(title: &str, description: &str, criteria: &[String], tags: &[String]) -> String {
    wg_intel::build_prompt(title, description, criteria, tags)
}

/// The error of a category outside the taxonomy.
#[must_use]
pub fn unknown_category(category: &str) -> WgError {
    WgError::new(
        ErrorCode::ValidationFailed,
        format!("unknown category '{category}'"),
    )
}

/// The error of a `classify promote` id that names no succeeded row, or any text that is not an id.
pub fn parse_id(text: &str, not_found: &str) -> Result<Id, WgError> {
    text.parse::<Id>()
        .map_err(|_| WgError::new(ErrorCode::NotFound, not_found))
}

/// The `{"category": ..., "model": ...}` of the former current classification of a confirmation.
#[must_use]
pub fn previous_of(category: &Option<String>, model: &Option<String>) -> Json {
    let mut object = Map::new();
    object.insert("category".to_owned(), text_or_null(category));
    object.insert("model".to_owned(), text_or_null(model));
    Json::Object(object)
}
