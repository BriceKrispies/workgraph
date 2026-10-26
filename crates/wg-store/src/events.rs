//! The append-only `events` log: appending drafts and reading records back.
//!
//! Mirrors `core/events/log.py`: [`append_event`] is `record_event`, [`query_events`] is
//! `query_events`, [`EventRecord`] is the dataclass of the same name, and reading upcasts old
//! payloads to the current schema version. The table rejects UPDATE and DELETE with triggers
//! (migration `0001`), so the store only ever inserts.

use rusqlite::types::Value as Sql;
use rusqlite::{Connection, params, params_from_iter};
use serde_json::{Value, json};
use wg_model::{EventDraft, JsonObject};
use wg_types::{ErrorCode, Id, Timestamp, WgError};

use crate::codec::{Cols, db_error, id_sql, json_text, opt_id_sql, ts_sql};

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
    pub correlation_id: Id,
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

fn decode_record(cols: &Cols<'_, '_>) -> rusqlite::Result<EventRecord> {
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
        correlation_id: cols.id("correlation_id")?,
        causation_id: cols.opt_id("causation_id")?,
        attempt_id: cols.opt_id("attempt_id")?,
        schema_version,
        data,
    })
}

const EVENT_COLUMNS: &str = "seq, id, workspace_id, event_type, entity_type, entity_id, actor, \
     actor_kind, occurred_at, correlation_id, causation_id, attempt_id, schema_version, data";

/// Appends one event inside the caller's transaction and returns its `seq`.
pub fn append_event(conn: &Connection, draft: &EventDraft) -> Result<i64, WgError> {
    let data = json_text(&Value::Object(draft.data()?))?
        .ok_or_else(|| WgError::new(ErrorCode::WorkgraphError, "event data serialised to null"))?;
    let mut statement = conn
        .prepare_cached(
            "INSERT INTO events (id, workspace_id, event_type, entity_type, entity_id, actor, \
             actor_kind, occurred_at, correlation_id, causation_id, attempt_id, schema_version, \
             data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )
        .map_err(|err| db_error(&err))?;
    statement
        .execute(params_from_iter([
            id_sql(draft.id),
            opt_id_sql(draft.workspace_id),
            Sql::Text(draft.event_type().to_owned()),
            Sql::Text(draft.entity_type().as_str().to_owned()),
            id_sql(draft.entity_id),
            Sql::Text(draft.actor.to_string()),
            Sql::Text(draft.actor.kind().as_str().to_owned()),
            ts_sql(draft.occurred_at),
            id_sql(draft.correlation_id),
            opt_id_sql(draft.causation_id),
            opt_id_sql(draft.attempt_id),
            Sql::Integer(i64::from(draft.schema_version())),
            Sql::Text(data),
        ]))
        .map_err(|err| db_error(&err))?;
    Ok(conn.last_insert_rowid())
}

/// The filters of Python's `query_events`. Unset fields do not filter; an empty `entity_ids` or
/// `event_types` does not filter either (Python treats an empty sequence as unset).
#[derive(Debug, Clone, PartialEq)]
pub struct EventQuery {
    pub workspace_id: Option<Id>,
    pub entity_type: Option<String>,
    pub entity_ids: Vec<Id>,
    pub event_types: Vec<String>,
    pub event_type_prefix: Option<String>,
    pub actor: Option<String>,
    pub attempt_id: Option<Id>,
    pub correlation_id: Option<Id>,
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

/// Runs an [`EventQuery`].
pub fn query_events(conn: &Connection, query: &EventQuery) -> Result<Vec<EventRecord>, WgError> {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Sql> = Vec::new();
    let bind = |args: &mut Vec<Sql>, value: Sql| {
        args.push(value);
        args.len()
    };
    if let Some(id) = query.workspace_id {
        let n = bind(&mut args, id_sql(id));
        clauses.push(format!("workspace_id = ?{n}"));
    }
    if let Some(entity_type) = &query.entity_type {
        let n = bind(&mut args, Sql::Text(entity_type.clone()));
        clauses.push(format!("entity_type = ?{n}"));
    }
    if !query.entity_ids.is_empty() {
        let marks: Vec<String> = query
            .entity_ids
            .iter()
            .map(|id| format!("?{}", bind(&mut args, id_sql(*id))))
            .collect();
        clauses.push(format!("entity_id IN ({})", marks.join(", ")));
    }
    if !query.event_types.is_empty() {
        let marks: Vec<String> = query
            .event_types
            .iter()
            .map(|name| format!("?{}", bind(&mut args, Sql::Text(name.clone()))))
            .collect();
        clauses.push(format!("event_type IN ({})", marks.join(", ")));
    }
    if let Some(prefix) = query.event_type_prefix.as_ref().filter(|p| !p.is_empty()) {
        let n = bind(&mut args, Sql::Text(prefix.clone()));
        clauses.push(format!("event_type LIKE ?{n} || '%'"));
    }
    if let Some(actor) = &query.actor {
        let n = bind(&mut args, Sql::Text(actor.clone()));
        clauses.push(format!("actor = ?{n}"));
    }
    if let Some(id) = query.attempt_id {
        let n = bind(&mut args, id_sql(id));
        clauses.push(format!("attempt_id = ?{n}"));
    }
    if let Some(id) = query.correlation_id {
        let n = bind(&mut args, id_sql(id));
        clauses.push(format!("correlation_id = ?{n}"));
    }
    if let Some(seq) = query.after_seq {
        let n = bind(&mut args, Sql::Integer(seq));
        clauses.push(format!("seq > ?{n}"));
    }
    if let Some(at) = query.since {
        let n = bind(&mut args, ts_sql(at));
        clauses.push(format!("occurred_at >= ?{n}"));
    }
    if let Some(at) = query.until {
        let n = bind(&mut args, ts_sql(at));
        clauses.push(format!("occurred_at < ?{n}"));
    }
    let filter = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    let order = if query.descending { "DESC" } else { "ASC" };
    let limit = bind(&mut args, Sql::Integer(query.limit));
    let sql =
        format!("SELECT {EVENT_COLUMNS} FROM events{filter} ORDER BY seq {order} LIMIT ?{limit}");
    let mut statement = conn.prepare(&sql).map_err(|err| db_error(&err))?;
    let rows = statement
        .query_map(params_from_iter(args), |row| decode_record(&Cols::new(row)))
        .map_err(|err| db_error(&err))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| db_error(&err))
}

/// `latest_seq`: the highest `seq`, or 0 for an empty log.
pub fn latest_seq(conn: &Connection) -> Result<i64, WgError> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM events",
        params![],
        |row| row.get(0),
    )
    .map_err(|err| db_error(&err))
}
