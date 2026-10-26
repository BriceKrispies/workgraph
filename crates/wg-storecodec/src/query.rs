//! The SELECT for an [`EventQuery`] (Python's `query_events`) and `latest_seq`.

use crate::codec::{Raw, id_sql, ts_sql};
use crate::events::{EVENT_COLUMNS, EventQuery};
use crate::sql::Statement;

impl EventQuery {
    /// The SELECT for these filters, columns in [`EVENT_COLUMNS`] order.
    pub fn statement(&self) -> Statement {
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<Raw> = Vec::new();
        let bind = |args: &mut Vec<Raw>, value: Raw| {
            args.push(value);
            args.len()
        };
        if let Some(id) = self.workspace_id {
            let n = bind(&mut args, id_sql(id));
            clauses.push(format!("workspace_id = ?{n}"));
        }
        if let Some(entity_type) = &self.entity_type {
            let n = bind(&mut args, Raw::Text(entity_type.clone()));
            clauses.push(format!("entity_type = ?{n}"));
        }
        if !self.entity_ids.is_empty() {
            let marks: Vec<String> = self
                .entity_ids
                .iter()
                .map(|id| format!("?{}", bind(&mut args, id_sql(*id))))
                .collect();
            clauses.push(format!("entity_id IN ({})", marks.join(", ")));
        }
        if !self.event_types.is_empty() {
            let marks: Vec<String> = self
                .event_types
                .iter()
                .map(|name| format!("?{}", bind(&mut args, Raw::Text(name.clone()))))
                .collect();
            clauses.push(format!("event_type IN ({})", marks.join(", ")));
        }
        if let Some(prefix) = self.event_type_prefix.as_ref().filter(|p| !p.is_empty()) {
            let n = bind(&mut args, Raw::Text(prefix.clone()));
            clauses.push(format!("event_type LIKE ?{n} || '%'"));
        }
        if let Some(actor) = &self.actor {
            let n = bind(&mut args, Raw::Text(actor.clone()));
            clauses.push(format!("actor = ?{n}"));
        }
        if let Some(id) = self.attempt_id {
            let n = bind(&mut args, id_sql(id));
            clauses.push(format!("attempt_id = ?{n}"));
        }
        if let Some(id) = self.correlation_id {
            let n = bind(&mut args, id_sql(id));
            clauses.push(format!("correlation_id = ?{n}"));
        }
        if let Some(seq) = self.after_seq {
            let n = bind(&mut args, Raw::Int(seq));
            clauses.push(format!("seq > ?{n}"));
        }
        if let Some(at) = self.since {
            let n = bind(&mut args, ts_sql(at));
            clauses.push(format!("occurred_at >= ?{n}"));
        }
        if let Some(at) = self.until {
            let n = bind(&mut args, ts_sql(at));
            clauses.push(format!("occurred_at < ?{n}"));
        }
        let filter = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        let order = if self.descending { "DESC" } else { "ASC" };
        let limit = bind(&mut args, Raw::Int(self.limit));
        Statement {
            sql: format!(
                "SELECT {} FROM events{filter} ORDER BY seq {order} LIMIT ?{limit}",
                EVENT_COLUMNS.join(", ")
            ),
            params: args,
        }
    }
}

/// `latest_seq`: the highest `seq`, or 0 for an empty log.
pub fn latest_seq_statement() -> Statement {
    Statement {
        sql: "SELECT COALESCE(MAX(seq), 0) FROM events".to_owned(),
        params: Vec::new(),
    }
}
