//! SQL text, parameters and typed rows for the classification family (SH-11): a task's
//! classification history, the lookup of a stored succeeded classification for an input, gold
//! labels, evaluations, and the succeeded classifications of a workspace joined to their task keys.
//!
//! Like [`wg_storecodec::task_sql`], every statement is a value ([`Statement`]); `wg-store` binds and runs
//! it and decodes the rows through the table codecs. Lists have a fixed `ORDER BY`, with `id`
//! breaking ties between rows that share a timestamp, so results are deterministic.

use wg_model::Table;
use wg_model::entity::Classification;
use wg_types::{ErrorCode, Id, WgError};

use wg_storecodec::codec::{Cols, Raw, id_sql, text_sql};
use wg_storecodec::rows::{TableRow, columns};
use wg_storecodec::sql::{Statement, select_list};

/// The classifications of `task_id`, newest first (`ClassificationService.get`).
pub fn classification_history(task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"classifications\" WHERE \"task_id\" = ?1 \
             ORDER BY \"created_at\" DESC, \"id\"",
            select_list(Table::Classification, None)
        ),
        params: vec![id_sql(task_id)],
    }
}

/// A succeeded classification of `task_id` made by `provider`/`model` under `prompt_version` for
/// the input hash `input_hash` (the reuse check of `run_classification`). At most one row.
pub fn succeeded_classification(
    task_id: Id,
    provider: &str,
    model: &str,
    prompt_version: &str,
    input_hash: &str,
) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"classifications\" WHERE \"task_id\" = ?1 AND \"provider\" = ?2 \
             AND \"model\" = ?3 AND \"prompt_version\" = ?4 AND \"input_hash\" = ?5 \
             AND \"status\" = ?6 LIMIT 1",
            select_list(Table::Classification, None)
        ),
        params: vec![
            id_sql(task_id),
            text_sql(provider),
            text_sql(model),
            text_sql(prompt_version),
            text_sql(input_hash),
            text_sql("succeeded"),
        ],
    }
}

/// The gold labels of a workspace, oldest first.
pub fn gold_labels(workspace_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"classification_gold_labels\" WHERE \"workspace_id\" = ?1 \
             ORDER BY \"created_at\", \"id\"",
            select_list(Table::ClassificationGoldLabel, None)
        ),
        params: vec![id_sql(workspace_id)],
    }
}

/// The evaluations of a workspace, newest first.
pub fn classification_evaluations(workspace_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"classification_evaluations\" WHERE \"workspace_id\" = ?1 \
             ORDER BY \"created_at\" DESC, \"id\"",
            select_list(Table::ClassificationEvaluation, None)
        ),
        params: vec![id_sql(workspace_id)],
    }
}

/// The succeeded classifications of a workspace, each with its task's key (the join behind
/// `disagreements`). Ordered by task number, then classification time.
pub fn classifications_with_task_keys(workspace_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {}, t.\"key\" FROM \"classifications\" AS c \
             JOIN \"tasks\" AS t ON t.\"id\" = c.\"task_id\" \
             WHERE t.\"workspace_id\" = ?1 AND c.\"status\" = ?2 \
             ORDER BY t.\"number\", c.\"created_at\", c.\"id\"",
            select_list(Table::Classification, Some("c"))
        ),
        params: vec![id_sql(workspace_id), text_sql("succeeded")],
    }
}

/// One succeeded classification with the key of its task.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassificationWithKey {
    pub classification: Classification,
    pub task_key: String,
}

impl ClassificationWithKey {
    /// Decodes a row of [`classifications_with_task_keys`]: the classification's columns, then
    /// `task_key`.
    pub fn from_raw(values: &[Raw]) -> Result<Self, WgError> {
        let width = columns(Table::Classification).len();
        let (Some(own), Some(rest)) = (values.get(..width), values.get(width..)) else {
            return Err(WgError::new(
                ErrorCode::WorkgraphError,
                "classification join row is shorter than the classification columns",
            ));
        };
        Ok(ClassificationWithKey {
            classification: Classification::from_raw(own)?,
            task_key: Cols::new(&["task_key"], rest).text("task_key")?,
        })
    }
}
