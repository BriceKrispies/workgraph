//! SQL text for the trial runner (C081): the subagent runs whose telemetry names a trial.

use wg_model::Table;
use wg_types::Id;

use wg_rows::codec::{id_sql, text_sql};
use wg_rows::sql::{Statement, select_list};

/// The subagent runs whose telemetry text mentions `trial_id`, the most recently stopped first
/// (`_latest_run`; a run that never stopped sorts last, and the id breaks ties).
pub fn runs_naming_trial(trial_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"subagent_runs\" WHERE CAST(\"telemetry\" AS TEXT) LIKE ?1 \
             ORDER BY \"stopped_at\" IS NULL, \"stopped_at\" DESC, \"id\"",
            select_list(Table::SubagentRun, None)
        ),
        params: vec![text_sql(&format!("%{trial_id}%"))],
    }
}

/// The subagent run `run_id`.
pub fn run_by_id(run_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"subagent_runs\" WHERE \"id\" = ?1",
            select_list(Table::SubagentRun, None)
        ),
        params: vec![id_sql(run_id)],
    }
}
