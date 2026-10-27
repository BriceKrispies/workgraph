//! The statements the run flow reads that no store call serves yet: the attempts of every task in
//! some statuses (the scheduler's counts) and the `agent_run` jobs.

use wg_model::Table;
use wg_storecodec::codec::text_sql;
use wg_storecodec::sql::{quote, select_list};
use wg_storecodec::{Raw, Statement};
use wg_types::AttemptStatus;

/// The attempts in any of `statuses`, in every workspace, oldest first.
pub fn attempts_in(statuses: &[AttemptStatus]) -> Statement {
    let marks: Vec<String> = (1..=statuses.len()).map(|n| format!("?{n}")).collect();
    let params: Vec<Raw> = statuses.iter().map(|s| text_sql(s.as_str())).collect();
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE {} IN ({}) ORDER BY {}, {}",
            select_list(Table::AgentAttempt, None),
            quote(Table::AgentAttempt.name()),
            quote("status"),
            marks.join(", "),
            quote("created_at"),
            quote("number"),
        ),
        params,
    }
}
