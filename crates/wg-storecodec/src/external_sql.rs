//! SQL text and parameters for external items (SH-14): the tracker mirror rows that tie a task to
//! an issue in a tracker connection. The lookups the tracker services make by task and by external
//! id, the per-connection list the sync walks, the counts the tracker status shows, and the joined
//! query that names the pull request repositories no repository row resolves.
//!
//! Every SELECT lists the table's [`columns`] in schema order, so its rows decode through the same
//! `TableRow` codecs as a fetch by key.

use wg_model::Table;
use wg_types::Id;

use crate::codec::{id_sql, text_sql};
use crate::sql::{Statement, select_list};

/// The kind a task link has when it is a GitHub pull request (Python `PULL_REQUEST_KIND`).
pub const PULL_REQUEST_KIND: &str = "pull_request";

/// The external item of `task_id` (unique per task), if the task is tracked.
pub fn external_item_by_task(task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"external_items\" WHERE \"task_id\" = ?1",
            select_list(Table::ExternalItem, None)
        ),
        params: vec![id_sql(task_id)],
    }
}

/// The external item of `connection` with the tracker's own id `external_id` (unique per
/// connection).
pub fn external_item_by_external_id(connection: &str, external_id: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"external_items\" WHERE \"connection\" = ?1 AND \"external_id\" = ?2",
            select_list(Table::ExternalItem, None)
        ),
        params: vec![text_sql(connection), text_sql(external_id)],
    }
}

/// Every external item of `connection`, ordered by the tracker's id. The Python query has no
/// `ORDER BY`; `external_id` is unique per connection, so the order is total.
pub fn external_items_of_connection(connection: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"external_items\" WHERE \"connection\" = ?1 ORDER BY \"external_id\"",
            select_list(Table::ExternalItem, None)
        ),
        params: vec![text_sql(connection)],
    }
}

/// The number of external items of `connection` (one row, one integer column).
pub fn tracked_count(connection: &str) -> Statement {
    Statement {
        sql: "SELECT COUNT(*) FROM \"external_items\" WHERE \"connection\" = ?1".to_owned(),
        params: vec![text_sql(connection)],
    }
}

/// The number of external items of `connection` that a full sync no longer found (one row, one
/// integer column).
pub fn missing_count(connection: &str) -> Statement {
    Statement {
        sql: "SELECT COUNT(*) FROM \"external_items\" WHERE \"connection\" = ?1 \
              AND \"missing_since\" IS NOT NULL"
            .to_owned(),
        params: vec![text_sql(connection)],
    }
}

/// The distinct `org/repo` names of the pull request links of `connection`'s tracked tasks that no
/// repository row resolves, in name order (one text column).
pub fn unresolved_pull_request_repos(connection: &str) -> Statement {
    Statement {
        sql: "SELECT DISTINCT \"task_links\".\"repo_full_name\" \
              FROM \"external_items\" JOIN \"task_links\" \
              ON \"task_links\".\"task_id\" = \"external_items\".\"task_id\" \
              WHERE \"external_items\".\"connection\" = ?1 \
              AND \"task_links\".\"kind\" = ?2 \
              AND \"task_links\".\"repository_id\" IS NULL \
              AND \"task_links\".\"repo_full_name\" <> '' \
              ORDER BY \"task_links\".\"repo_full_name\""
            .to_owned(),
        params: vec![text_sql(connection), text_sql(PULL_REQUEST_KIND)],
    }
}
