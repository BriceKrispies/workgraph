//! Reads for the automation family (SH-12): automation rules and their runs, notifications,
//! reports and analytics findings, as free functions over the store session.
//!
//! The statements come from [`crate::spec::sql`] as values; each function runs them through the
//! session and decodes the rows with the table codecs. Writes go through the generic
//! `Session::apply_write` and `Session::apply_decision` like every other family.

use wg_model::entity::{AutomationRule, AutomationRun, Finding, Notification, Report};
use wg_store::{EventQuery, EventRecord, Raw, Session, many, one};
use wg_types::{Id, Timestamp, WgError};

use crate::spec::sql::{self, Page, RuleRun};

/// The event with `id`, if one exists (`select(Event).where(Event.id == id)`).
pub fn event(session: &mut Session<'_>, id: Id) -> Result<Option<EventRecord>, WgError> {
    let query = EventQuery {
        id: Some(id),
        limit: 1,
        ..EventQuery::default()
    };
    Ok(session.query_events(&query)?.into_iter().next())
}

/// The events after cursor `after_seq`, oldest first, at most `limit` of them: what the
/// automation cursor has not yet processed (`Event.seq > cursor`).
pub fn events_after(
    session: &mut Session<'_>,
    after_seq: i64,
    limit: i64,
) -> Result<Vec<EventRecord>, WgError> {
    let query = EventQuery {
        after_seq: Some(after_seq),
        limit,
        ..EventQuery::default()
    };
    session.query_events(&query)
}

/// The rule in `workspace_id` named `name`, if any.
pub fn rule_by_name(
    session: &mut Session<'_>,
    workspace_id: Id,
    name: &str,
) -> Result<Option<AutomationRule>, WgError> {
    one(session, &sql::rule_by_name(workspace_id, name))
}

/// The rules of `workspace_id`, ordered by name.
pub fn rules(session: &mut Session<'_>, workspace_id: Id) -> Result<Vec<AutomationRule>, WgError> {
    many(session, &sql::rules(workspace_id))
}

/// The enabled rules, in every workspace, whose trigger type is `trigger_type`.
pub fn enabled_rules(
    session: &mut Session<'_>,
    trigger_type: &str,
) -> Result<Vec<AutomationRule>, WgError> {
    many(session, &sql::enabled_rules_of_trigger(trigger_type))
}

/// The run of `rule_id` for `trigger_key`, if one exists.
pub fn run_by_trigger(
    session: &mut Session<'_>,
    rule_id: Id,
    trigger_key: &str,
) -> Result<Option<AutomationRun>, WgError> {
    one(session, &sql::run_by_trigger(rule_id, trigger_key))
}

/// The oldest run, in any rule, with `trigger_key`, if one exists.
pub fn run_by_trigger_key(
    session: &mut Session<'_>,
    trigger_key: &str,
) -> Result<Option<AutomationRun>, WgError> {
    one(session, &sql::run_by_trigger_key(trigger_key))
}

/// The runs of `workspace_id`, newest first, narrowed to `rule_id` and `status` when given.
pub fn runs(
    session: &mut Session<'_>,
    workspace_id: Id,
    rule_id: Option<Id>,
    status: Option<&str>,
    page: Page,
) -> Result<Vec<AutomationRun>, WgError> {
    many(session, &sql::runs(workspace_id, rule_id, status, page))
}

/// The same runs as [`runs`], each with the name of its rule.
pub fn runs_with_rule(
    session: &mut Session<'_>,
    workspace_id: Id,
    rule_id: Option<Id>,
    status: Option<&str>,
    page: Page,
) -> Result<Vec<RuleRun>, WgError> {
    session
        .rows(&sql::runs_with_rule(workspace_id, rule_id, status, page))?
        .iter()
        .map(|values| sql::decode_rule_run(values))
        .collect()
}

/// How many live, non-dry runs of `rule_id` were created after `since`.
pub fn live_run_count(
    session: &mut Session<'_>,
    rule_id: Id,
    since: Timestamp,
) -> Result<i64, WgError> {
    match session
        .rows(&sql::live_run_count(rule_id, since))?
        .first()
        .and_then(|row| row.first())
    {
        Some(Raw::Int(count)) => Ok(*count),
        _ => Err(WgError::workgraph(
            "COUNT(*) over automation_runs returned no integer",
        )),
    }
}

/// The notifications of `workspace_id`, newest first; only unread ones when `unread_only`.
pub fn notifications(
    session: &mut Session<'_>,
    workspace_id: Id,
    unread_only: bool,
    page: Page,
) -> Result<Vec<Notification>, WgError> {
    many(
        session,
        &sql::notifications(workspace_id, unread_only, page),
    )
}

/// The reports of `workspace_id`, newest first.
pub fn reports(
    session: &mut Session<'_>,
    workspace_id: Id,
    page: Page,
) -> Result<Vec<Report>, WgError> {
    many(session, &sql::reports(workspace_id, page))
}

/// The finding of `workspace_id` with `fingerprint`, if one exists.
pub fn finding_by_fingerprint(
    session: &mut Session<'_>,
    workspace_id: Id,
    fingerprint: &str,
) -> Result<Option<Finding>, WgError> {
    one(
        session,
        &sql::finding_by_fingerprint(workspace_id, fingerprint),
    )
}
