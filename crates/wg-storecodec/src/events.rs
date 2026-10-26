//! The `events` log as values: event rows, queries and their statements.
//!
//! Mirrors `core/events/log.py`: [`append_statement`] is `record_event`, [`EventQuery::statement`]
//! is `query_events`, [`EventRecord`] is the dataclass of the same name, and decoding upcasts old
//! payloads to the current schema version. The table rejects UPDATE and DELETE with triggers
//! (migration `0001`), so the store only ever inserts.

use serde_json::{Value, json};
use wg_model::{EventDraft, JsonObject, StoredEvent};
use wg_types::{Correlation, ErrorCode, Id, Timestamp, WgError};

use crate::codec::{Cols, Raw, correlation_sql, id_sql, json_text, opt_id_sql, ts_sql};
use crate::sql::Statement;

impl From<EventRecord> for StoredEvent {
    /// The same row as the model's [`StoredEvent`]: every field carries over unchanged.
    fn from(record: EventRecord) -> StoredEvent {
        StoredEvent {
            seq: record.seq,
            id: record.id,
            workspace_id: record.workspace_id,
            event_type: record.event_type,
            entity_type: record.entity_type,
            entity_id: record.entity_id,
            actor: record.actor,
            actor_kind: record.actor_kind,
            occurred_at: record.occurred_at,
            correlation_id: record.correlation_id,
            causation_id: record.causation_id,
            attempt_id: record.attempt_id,
            schema_version: record.schema_version,
            data: record.data,
        }
    }
}

/// Python's `DEFAULT_SCHEMA_VERSION`.
const DEFAULT_SCHEMA_VERSION: i64 = 1;

/// Python's `EVENT_SCHEMA_VERSIONS`: the current `data` shape version of an event type.
fn current_version(event_type: &str) -> i64 {
    match event_type {
        "task.created" => 2,
        _ => DEFAULT_SCHEMA_VERSION,
    }
}

/// One stored event, after upcasting (Python's `EventRecord`).
///
/// `actor` and `actor_kind` stay text: the column holds whatever the writer recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct EventRecord {
    pub seq: i64,
    pub id: Id,
    pub workspace_id: Option<Id>,
    pub event_type: String,
    pub entity_type: String,
    pub entity_id: Id,
    pub actor: String,
    pub actor_kind: String,
    pub occurred_at: Timestamp,
    pub correlation_id: Correlation,
    pub causation_id: Option<Id>,
    pub attempt_id: Option<Id>,
    pub schema_version: i64,
    pub data: JsonObject,
}

impl EventRecord {
    /// `EventRecord.to_dict()`: `occurred_at` as Python's `isoformat()`, ids as text.
    pub fn to_dict(&self) -> Value {
        json!({
            "seq": self.seq,
            "id": self.id.to_string(),
            "workspace_id": self.workspace_id.map(|id| id.to_string()),
            "event_type": self.event_type,
            "entity_type": self.entity_type,
            "entity_id": self.entity_id.to_string(),
            "actor": self.actor,
            "actor_kind": self.actor_kind,
            "occurred_at": self.occurred_at.format_isoformat(),
            "correlation_id": self.correlation_id.to_string(),
            "causation_id": self.causation_id.map(|id| id.to_string()),
            "attempt_id": self.attempt_id.map(|id| id.to_string()),
            "schema_version": self.schema_version,
            "data": Value::Object(self.data.clone()),
        })
    }
}

/// `upcast`: converts a stored payload to the current version of its type.
fn upcast(event_type: &str, mut version: i64, mut data: JsonObject) -> (i64, JsonObject) {
    let target = current_version(event_type);
    while version < target {
        match (event_type, version) {
            ("task.created", 1) => data = task_created_v1_to_v2(data),
            _ => break,
        }
        version += 1;
    }
    (version, data)
}

/// v1 stored tags as a comma-separated string and had no acceptance criteria.
fn task_created_v1_to_v2(mut data: JsonObject) -> JsonObject {
    let tags = data
        .get("tags")
        .cloned()
        .unwrap_or_else(|| Value::String(String::new()));
    if let Value::String(text) = tags {
        let list = text
            .split(',')
            .map(py_strip)
            .filter(|tag| !tag.is_empty())
            .map(|tag| Value::String(tag.to_owned()))
            .collect();
        data.insert("tags".to_owned(), Value::Array(list));
    }
    if !data.contains_key("acceptance_criteria") {
        data.insert("acceptance_criteria".to_owned(), Value::Array(Vec::new()));
    }
    data
}

/// Python `str.strip()`: removes Unicode whitespace from both ends.
fn py_strip(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// Decodes one row selected in [`EVENT_COLUMNS`] order.
pub fn decode_event(values: &[Raw]) -> Result<EventRecord, WgError> {
    let cols = Cols::new(EVENT_COLUMNS, values);
    let event_type = cols.text("event_type")?;
    let (schema_version, data) = upcast(
        &event_type,
        cols.int("schema_version")?,
        cols.object("data")?,
    );
    Ok(EventRecord {
        seq: cols.int("seq")?,
        id: cols.id("id")?,
        workspace_id: cols.opt_id("workspace_id")?,
        event_type,
        entity_type: cols.text("entity_type")?,
        entity_id: cols.id("entity_id")?,
        actor: cols.text("actor")?,
        actor_kind: cols.text("actor_kind")?,
        occurred_at: cols.ts("occurred_at")?,
        correlation_id: cols.correlation("correlation_id")?,
        causation_id: cols.opt_id("causation_id")?,
        attempt_id: cols.opt_id("attempt_id")?,
        schema_version,
        data,
    })
}

/// The columns of `events`, in schema order.
pub const EVENT_COLUMNS: &[&str] = &[
    "seq",
    "id",
    "workspace_id",
    "event_type",
    "entity_type",
    "entity_id",
    "actor",
    "actor_kind",
    "occurred_at",
    "correlation_id",
    "causation_id",
    "attempt_id",
    "schema_version",
    "data",
];

/// The insert for one event; `seq` is assigned by the database.
pub fn append_statement(draft: &EventDraft) -> Result<Statement, WgError> {
    let data = json_text(&Value::Object(draft.data()?))?
        .ok_or_else(|| WgError::new(ErrorCode::WorkgraphError, "event data serialised to null"))?;
    Ok(Statement {
        sql: "INSERT INTO events (id, workspace_id, event_type, entity_type, entity_id, actor, \
              actor_kind, occurred_at, correlation_id, causation_id, attempt_id, schema_version, \
              data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
            .to_owned(),
        params: vec![
            id_sql(draft.id),
            opt_id_sql(draft.workspace_id),
            Raw::Text(draft.event_type().to_owned()),
            Raw::Text(draft.entity_type().as_str().to_owned()),
            id_sql(draft.entity_id),
            Raw::Text(draft.actor.to_string()),
            Raw::Text(draft.actor.kind().as_str().to_owned()),
            ts_sql(draft.occurred_at),
            correlation_sql(&draft.correlation_id),
            opt_id_sql(draft.causation_id),
            opt_id_sql(draft.attempt_id),
            Raw::Int(i64::from(draft.schema_version())),
            Raw::Text(data),
        ],
    })
}

/// The filters of Python's `query_events`. Unset fields do not filter; an empty `entity_ids` or
/// `event_types` does not filter either (Python treats an empty sequence as unset).
#[derive(Debug, Clone, PartialEq)]
pub struct EventQuery {
    pub workspace_id: Option<Id>,
    /// The event with this id (`Event.id == id`), for lookups by the id a caller was given.
    pub id: Option<Id>,
    pub entity_type: Option<String>,
    pub entity_ids: Vec<Id>,
    pub event_types: Vec<String>,
    pub event_type_prefix: Option<String>,
    pub actor: Option<String>,
    pub attempt_id: Option<Id>,
    pub correlation_id: Option<Correlation>,
    pub after_seq: Option<i64>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    pub limit: i64,
    pub descending: bool,
}

impl Default for EventQuery {
    /// No filters, ascending by `seq`, limit 200.
    fn default() -> Self {
        EventQuery {
            workspace_id: None,
            id: None,
            entity_type: None,
            entity_ids: Vec::new(),
            event_types: Vec::new(),
            event_type_prefix: None,
            actor: None,
            attempt_id: None,
            correlation_id: None,
            after_seq: None,
            since: None,
            until: None,
            limit: 200,
            descending: false,
        }
    }
}

impl EventQuery {
    /// Events of one entity, oldest first.
    pub fn by_entity(entity_type: &str, entity_id: Id) -> Self {
        EventQuery {
            entity_type: Some(entity_type.to_owned()),
            entity_ids: vec![entity_id],
            ..EventQuery::default()
        }
    }

    /// Events of one workspace with `seq` greater than `after_seq`, oldest first.
    pub fn after(workspace_id: Id, after_seq: i64) -> Self {
        EventQuery {
            workspace_id: Some(workspace_id),
            after_seq: Some(after_seq),
            ..EventQuery::default()
        }
    }
}
