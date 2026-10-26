//! [`AutomationSlice`]: automation rules and runs, notifications, reports and findings.

use rpds::RedBlackTreeMapSync;
use wg_types::Id;

use crate::entity::{AutomationRule, AutomationRun, Finding, Notification, Report};
use crate::write::{Row, RowKey, Write};

/// Loaded rows of the automation family, keyed by primary key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AutomationSlice {
    pub rules: RedBlackTreeMapSync<Id, AutomationRule>,
    pub runs: RedBlackTreeMapSync<Id, AutomationRun>,
    pub notifications: RedBlackTreeMapSync<Id, Notification>,
    pub reports: RedBlackTreeMapSync<Id, Report>,
    pub findings: RedBlackTreeMapSync<Id, Finding>,
}

impl AutomationSlice {
    pub fn rule(&self, id: Id) -> Option<&AutomationRule> {
        self.rules.get(&id)
    }

    /// The runs of a rule, by run id.
    pub fn runs_of(&self, rule_id: Id) -> impl Iterator<Item = &AutomationRun> {
        self.runs.values().filter(move |row| row.rule_id == rule_id)
    }

    /// The slice after `write`. Writes to tables this slice does not hold leave it unchanged.
    #[must_use]
    pub fn apply(&self, write: &Write) -> Self {
        match write {
            Write::Insert(row) | Write::Update { row, .. } => match row {
                Row::AutomationRule(row) => AutomationSlice {
                    rules: self.rules.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::AutomationRun(row) => AutomationSlice {
                    runs: self.runs.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::Notification(row) => AutomationSlice {
                    notifications: self.notifications.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::Report(row) => AutomationSlice {
                    reports: self.reports.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::Finding(row) => AutomationSlice {
                    findings: self.findings.insert(row.id, row.clone()),
                    ..self.clone()
                },
                _ => self.clone(),
            },
            Write::Delete(key) => match key {
                RowKey::AutomationRule(id) => AutomationSlice {
                    rules: self.rules.remove(id),
                    ..self.clone()
                },
                RowKey::AutomationRun(id) => AutomationSlice {
                    runs: self.runs.remove(id),
                    ..self.clone()
                },
                RowKey::Notification(id) => AutomationSlice {
                    notifications: self.notifications.remove(id),
                    ..self.clone()
                },
                RowKey::Report(id) => AutomationSlice {
                    reports: self.reports.remove(id),
                    ..self.clone()
                },
                RowKey::Finding(id) => AutomationSlice {
                    findings: self.findings.remove(id),
                    ..self.clone()
                },
                _ => self.clone(),
            },
        }
    }
}
