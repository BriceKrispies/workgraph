//! [`VerifySlice`]: verification configs, runs, check results and task acceptances.

use rpds::RedBlackTreeMapSync;
use wg_types::Id;

use crate::entity::{TaskAcceptance, VerificationCheckResult, VerificationConfig, VerificationRun};
use crate::write::{Row, RowKey, Write};

/// Loaded rows of the verification family, keyed by primary key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VerifySlice {
    pub configs: RedBlackTreeMapSync<Id, VerificationConfig>,
    pub runs: RedBlackTreeMapSync<Id, VerificationRun>,
    pub check_results: RedBlackTreeMapSync<Id, VerificationCheckResult>,
    pub acceptances: RedBlackTreeMapSync<Id, TaskAcceptance>,
}

impl VerifySlice {
    pub fn config(&self, id: Id) -> Option<&VerificationConfig> {
        self.configs.get(&id)
    }

    pub fn run(&self, id: Id) -> Option<&VerificationRun> {
        self.runs.get(&id)
    }

    /// The check results of a run, by result id.
    pub fn check_results_of(&self, run_id: Id) -> impl Iterator<Item = &VerificationCheckResult> {
        self.check_results
            .values()
            .filter(move |row| row.run_id == run_id)
    }

    /// The acceptance decisions of a task, by acceptance id.
    pub fn acceptances_of(&self, task_id: Id) -> impl Iterator<Item = &TaskAcceptance> {
        self.acceptances
            .values()
            .filter(move |row| row.task_id == task_id)
    }

    /// The slice after `write`. Writes to tables this slice does not hold leave it unchanged.
    #[must_use]
    pub fn apply(&self, write: &Write) -> Self {
        match write {
            Write::Insert(row) | Write::Update { row, .. } => match row {
                Row::VerificationConfig(row) => VerifySlice {
                    configs: self.configs.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::VerificationRun(row) => VerifySlice {
                    runs: self.runs.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::VerificationCheckResult(row) => VerifySlice {
                    check_results: self.check_results.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::TaskAcceptance(row) => VerifySlice {
                    acceptances: self.acceptances.insert(row.id, row.clone()),
                    ..self.clone()
                },
                _ => self.clone(),
            },
            Write::Delete(key) => match key {
                RowKey::VerificationConfig(id) => VerifySlice {
                    configs: self.configs.remove(id),
                    ..self.clone()
                },
                RowKey::VerificationRun(id) => VerifySlice {
                    runs: self.runs.remove(id),
                    ..self.clone()
                },
                RowKey::VerificationCheckResult(id) => VerifySlice {
                    check_results: self.check_results.remove(id),
                    ..self.clone()
                },
                RowKey::TaskAcceptance(id) => VerifySlice {
                    acceptances: self.acceptances.remove(id),
                    ..self.clone()
                },
                _ => self.clone(),
            },
        }
    }
}
