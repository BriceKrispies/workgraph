//! Reads for the automation family (SH-12): automation rules and their runs, notifications,
//! reports and analytics findings.
//!
//! The statements come from `wg_storecodec::automation_sql` as values; this module only runs them
//! through the session and decodes the rows with the table codecs. Writes go through the generic
//! [`Session::apply_write`] and [`Session::apply_decision`] like every other family.

use wg_model::entity::{AutomationRule, AutomationRun, Finding, Notification, Report};
use wg_storecodec::automation_sql::{self, Page, RuleRun};
use wg_storecodec::{EventQuery, EventRecord, Raw};
use wg_types::{ErrorCode, Id, Timestamp, WgError};

use crate::Session;
use crate::query::{many, one};

impl Session<'_> {
    /// The event with `id`, if one exists (`select(Event).where(Event.id == id)`).
    pub fn automation_event(&mut self, id: Id) -> Result<Option<EventRecord>, WgError> {
        let query = EventQuery {
            id: Some(id),
            limit: 1,
            ..EventQuery::default()
        };
        Ok(self.query_events(&query)?.into_iter().next())
    }

    /// The events after cursor `after_seq`, oldest first, at most `limit` of them: what the
    /// automation cursor has not yet processed (`Event.seq > cursor`).
    pub fn automation_events_after(
        &mut self,
        after_seq: i64,
        limit: i64,
    ) -> Result<Vec<EventRecord>, WgError> {
        let query = EventQuery {
            after_seq: Some(after_seq),
            limit,
            ..EventQuery::default()
        };
        self.query_events(&query)
    }

    /// The rule in `workspace_id` named `name`, if any.
    pub fn automation_rule_by_name(
        &mut self,
        workspace_id: Id,
        name: &str,
    ) -> Result<Option<AutomationRule>, WgError> {
        one(self, &automation_sql::rule_by_name(workspace_id, name))
    }

    /// The rules of `workspace_id`, ordered by name.
    pub fn automation_rules(&mut self, workspace_id: Id) -> Result<Vec<AutomationRule>, WgError> {
        many(self, &automation_sql::rules(workspace_id))
    }

    /// The enabled rules, in every workspace, whose trigger type is `trigger_type`.
    pub fn enabled_automation_rules(
        &mut self,
        trigger_type: &str,
    ) -> Result<Vec<AutomationRule>, WgError> {
        many(
            self,
            &automation_sql::enabled_rules_of_trigger(trigger_type),
        )
    }

    /// The run of `rule_id` for `trigger_key`, if one exists.
    pub fn automation_run_by_trigger(
        &mut self,
        rule_id: Id,
        trigger_key: &str,
    ) -> Result<Option<AutomationRun>, WgError> {
        one(self, &automation_sql::run_by_trigger(rule_id, trigger_key))
    }

    /// The oldest run, in any rule, with `trigger_key`, if one exists.
    pub fn automation_run_by_trigger_key(
        &mut self,
        trigger_key: &str,
    ) -> Result<Option<AutomationRun>, WgError> {
        one(self, &automation_sql::run_by_trigger_key(trigger_key))
    }

    /// The runs of `workspace_id`, newest first, narrowed to `rule_id` and `status` when given.
    pub fn automation_runs(
        &mut self,
        workspace_id: Id,
        rule_id: Option<Id>,
        status: Option<&str>,
        page: Page,
    ) -> Result<Vec<AutomationRun>, WgError> {
        many(
            self,
            &automation_sql::runs(workspace_id, rule_id, status, page),
        )
    }

    /// The same runs as [`Session::automation_runs`], each with the name of its rule.
    pub fn automation_runs_with_rule(
        &mut self,
        workspace_id: Id,
        rule_id: Option<Id>,
        status: Option<&str>,
        page: Page,
    ) -> Result<Vec<RuleRun>, WgError> {
        self.rows(&automation_sql::runs_with_rule(
            workspace_id,
            rule_id,
            status,
            page,
        ))?
        .iter()
        .map(|values| automation_sql::decode_rule_run(values))
        .collect()
    }

    /// How many live, non-dry runs of `rule_id` were created after `since`.
    pub fn automation_live_run_count(
        &mut self,
        rule_id: Id,
        since: Timestamp,
    ) -> Result<i64, WgError> {
        match self
            .rows(&automation_sql::live_run_count(rule_id, since))?
            .first()
            .and_then(|row| row.first())
        {
            Some(Raw::Int(count)) => Ok(*count),
            _ => Err(WgError::new(
                ErrorCode::WorkgraphError,
                "COUNT(*) over automation_runs returned no integer",
            )),
        }
    }

    /// The notifications of `workspace_id`, newest first; only unread ones when `unread_only`.
    pub fn notifications(
        &mut self,
        workspace_id: Id,
        unread_only: bool,
        page: Page,
    ) -> Result<Vec<Notification>, WgError> {
        many(
            self,
            &automation_sql::notifications(workspace_id, unread_only, page),
        )
    }

    /// The reports of `workspace_id`, newest first.
    pub fn reports(&mut self, workspace_id: Id, page: Page) -> Result<Vec<Report>, WgError> {
        many(self, &automation_sql::reports(workspace_id, page))
    }

    /// The finding of `workspace_id` with `fingerprint`, if one exists.
    pub fn finding_by_fingerprint(
        &mut self,
        workspace_id: Id,
        fingerprint: &str,
    ) -> Result<Option<Finding>, WgError> {
        one(
            self,
            &automation_sql::finding_by_fingerprint(workspace_id, fingerprint),
        )
    }
}
