//! Verification reads that base code shares: the workspace's verification configs (scope
//! resolution in the registry, tasks and worktrees), the newest run of a task and the task's
//! acceptance decisions (the acceptance gate in `wg-runtime`).
//!
//! The statements come from `wg_storecodec::acceptance_sql`; this module runs them and decodes the
//! rows through the table codecs. The rest of the verification family is `wg-verify-feature`.

use wg_model::entity::{TaskAcceptance, VerificationConfig, VerificationRun};
use wg_storecodec::TableRow;
use wg_storecodec::acceptance_sql;
use wg_types::{Id, WgError};

use crate::Session;

impl Session<'_> {
    /// The verification configs of `workspace_id`, by scope type and name.
    pub fn verification_configs(
        &mut self,
        workspace_id: Id,
    ) -> Result<Vec<VerificationConfig>, WgError> {
        self.rows(&acceptance_sql::configs_in_workspace(workspace_id))?
            .iter()
            .map(|values| VerificationConfig::from_raw(values))
            .collect()
    }

    /// The newest run of `task_id`, if the task has one.
    pub fn latest_verification_run(
        &mut self,
        task_id: Id,
    ) -> Result<Option<VerificationRun>, WgError> {
        self.rows(&acceptance_sql::latest_run_of_task(task_id))?
            .iter()
            .map(|values| VerificationRun::from_raw(values))
            .collect::<Result<Vec<_>, _>>()
            .map(|mut runs| runs.pop())
    }

    /// The acceptance decisions recorded for `task_id`, oldest first.
    pub fn task_acceptances(&mut self, task_id: Id) -> Result<Vec<TaskAcceptance>, WgError> {
        self.rows(&acceptance_sql::acceptances_of_task(task_id))?
            .iter()
            .map(|values| TaskAcceptance::from_raw(values))
            .collect()
    }
}
