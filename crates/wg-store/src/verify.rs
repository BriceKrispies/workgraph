//! Verification-family reads on a [`Session`] (SH-10): verification configs, runs, check results,
//! the failure history and task acceptances.
//!
//! The statements are built by `wg_storecodec::verification_sql` as values; this module only runs
//! them and decodes the rows. Writes go through the generic [`Session::apply_write`] and
//! [`Session::apply_decision`] like every other family.

use wg_model::entity::{
    TaskAcceptance, VerificationCheckResult, VerificationConfig, VerificationRun,
};
use wg_storecodec::verification_sql::{self, decode_failure};
use wg_storecodec::{Statement, TableRow};
use wg_types::{Id, WgError};

pub use wg_storecodec::verification_sql::{FailureFilter, FailureRow, Page, RunFilter};

use crate::Session;

impl Session<'_> {
    /// Runs a query and decodes every row through the table's codec.
    fn load_verification<T: TableRow>(&mut self, statement: &Statement) -> Result<Vec<T>, WgError> {
        self.rows(statement)?
            .iter()
            .map(|values| T::from_raw(values))
            .collect()
    }

    /// The verification config of `scope_type` and `scope_id` named `name` in `workspace_id`.
    pub fn verification_config(
        &mut self,
        workspace_id: Id,
        scope_type: &str,
        scope_id: Id,
        name: &str,
    ) -> Result<Option<VerificationConfig>, WgError> {
        Ok(self
            .load_verification::<VerificationConfig>(&verification_sql::config_by_scope(
                workspace_id,
                scope_type,
                scope_id,
                name,
            ))?
            .pop())
    }

    /// The verification configs of `workspace_id`, by scope type and name.
    pub fn verification_configs(
        &mut self,
        workspace_id: Id,
    ) -> Result<Vec<VerificationConfig>, WgError> {
        self.load_verification(&verification_sql::configs_in_workspace(workspace_id))
    }

    /// The newest run of `task_id`, if the task has one.
    pub fn latest_verification_run(
        &mut self,
        task_id: Id,
    ) -> Result<Option<VerificationRun>, WgError> {
        Ok(self
            .load_verification::<VerificationRun>(&verification_sql::latest_run_of_task(task_id))?
            .pop())
    }

    /// The runs of `workspace_id`, newest first, narrowed by `filter` and cut to `page`.
    pub fn verification_runs(
        &mut self,
        workspace_id: Id,
        filter: &RunFilter,
        page: Page,
    ) -> Result<Vec<VerificationRun>, WgError> {
        self.load_verification(&verification_sql::runs_in_workspace(
            workspace_id,
            filter,
            page,
        ))
    }

    /// The check results of `run_id`, in the order they were recorded.
    pub fn verification_check_results(
        &mut self,
        run_id: Id,
    ) -> Result<Vec<VerificationCheckResult>, WgError> {
        self.load_verification(&verification_sql::check_results_of_run(run_id))
    }

    /// The failed and errored check results of `workspace_id`, newest first, each with its run and
    /// task, narrowed by `filter` and cut to `page`.
    pub fn verification_failures(
        &mut self,
        workspace_id: Id,
        filter: &FailureFilter,
        page: Page,
    ) -> Result<Vec<FailureRow>, WgError> {
        self.rows(&verification_sql::failures(workspace_id, filter, page))?
            .iter()
            .map(|values| decode_failure(values))
            .collect()
    }

    /// The acceptance decisions recorded for `task_id`, oldest first.
    pub fn task_acceptances(&mut self, task_id: Id) -> Result<Vec<TaskAcceptance>, WgError> {
        self.load_verification(&verification_sql::acceptances_of_task(task_id))
    }
}
