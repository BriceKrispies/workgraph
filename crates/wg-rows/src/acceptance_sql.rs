//! SQL text and parameters for the reads the acceptance gate and the task layers share: the
//! verification configs of a workspace, the newest run of a task, and the task's acceptance
//! decisions. Base code reads them (`wg-runtime` acceptance, git, registry, tasks and worktrees);
//! the rest of the verification family lives in `wg-verify-feature`.
//!
//! Like the other `*_sql` modules, every statement is a value ([`Statement`]); `wg-store` binds and
//! runs it. Every SELECT lists the table's columns in schema order, so its rows decode through the
//! same `TableRow` codecs as a fetch by key.

use wg_model::Table;
use wg_types::Id;

use crate::codec::id_sql;
use crate::sql::{Statement, select_list};

/// The verification configs of a workspace, ordered by scope type and name.
pub fn configs_in_workspace(workspace_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"verification_configs\" WHERE \"workspace_id\" = ?1 \
             ORDER BY \"scope_type\", \"name\", \"id\"",
            select_list(Table::VerificationConfig, None)
        ),
        params: vec![id_sql(workspace_id)],
    }
}

/// The newest run of a task (the one acceptance judges); at most one row.
pub fn latest_run_of_task(task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"verification_runs\" WHERE \"task_id\" = ?1 \
             ORDER BY \"created_at\" DESC, \"id\" DESC LIMIT 1",
            select_list(Table::VerificationRun, None)
        ),
        params: vec![id_sql(task_id)],
    }
}

/// The acceptance decisions of a task, oldest first.
pub fn acceptances_of_task(task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"task_acceptances\" WHERE \"task_id\" = ?1 \
             ORDER BY \"created_at\", \"id\"",
            select_list(Table::TaskAcceptance, None)
        ),
        params: vec![id_sql(task_id)],
    }
}
