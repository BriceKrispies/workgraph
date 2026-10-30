//! Verification-family reads on a [`Session`] (SH-10): verification configs of one scope, runs,
//! check results and the failure history.
//!
//! The statements come from [`crate::spec::sql`]; these functions only run them and decode the
//! rows. The workspace configs, the newest run and the acceptances base code reads are methods of
//! `wg_store::Session` (its `acceptance` module). Writes go through `Session::apply_write`.

use wg_model::entity::{VerificationCheckResult, VerificationConfig, VerificationRun};
use wg_rows::{Statement, TableRow};
use wg_store::Session;
use wg_types::{Id, WgError};

use crate::spec::sql::{self, FailureFilter, FailureRow, Page, RunFilter, decode_failure};

/// Runs a query and decodes every row through the table's codec.
fn load<T: TableRow>(session: &mut Session<'_>, statement: &Statement) -> Result<Vec<T>, WgError> {
    session
        .rows(statement)?
        .iter()
        .map(|values| T::from_raw(values))
        .collect()
}

/// The verification config of `scope_type` and `scope_id` named `name` in `workspace_id`.
pub fn verification_config(
    session: &mut Session<'_>,
    workspace_id: Id,
    scope_type: &str,
    scope_id: Id,
    name: &str,
) -> Result<Option<VerificationConfig>, WgError> {
    Ok(load::<VerificationConfig>(
        session,
        &sql::config_by_scope(workspace_id, scope_type, scope_id, name),
    )?
    .pop())
}

/// The runs of `workspace_id`, newest first, narrowed by `filter` and cut to `page`.
pub fn verification_runs(
    session: &mut Session<'_>,
    workspace_id: Id,
    filter: &RunFilter,
    page: Page,
) -> Result<Vec<VerificationRun>, WgError> {
    load(session, &sql::runs_in_workspace(workspace_id, filter, page))
}

/// The check results of `run_id`, in the order they were recorded.
pub fn verification_check_results(
    session: &mut Session<'_>,
    run_id: Id,
) -> Result<Vec<VerificationCheckResult>, WgError> {
    load(session, &sql::check_results_of_run(run_id))
}

/// The failed and errored check results of `workspace_id`, newest first, each with its run and
/// task, narrowed by `filter` and cut to `page`.
pub fn verification_failures(
    session: &mut Session<'_>,
    workspace_id: Id,
    filter: &FailureFilter,
    page: Page,
) -> Result<Vec<FailureRow>, WgError> {
    session
        .rows(&sql::failures(workspace_id, filter, page))?
        .iter()
        .map(|values| decode_failure(values))
        .collect()
}
