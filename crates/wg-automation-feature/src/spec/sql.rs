//! SQL text and parameters for the automation family (SH-12): automation rules and their runs,
//! notifications, reports and analytics findings.
//!
//! Every SELECT lists the table's [`columns`] in schema order, so its rows decode through the same
//! `TableRow` codecs as a fetch by key. Lists have a fixed `ORDER BY` (with the primary key as the
//! last tie-break) and an explicit [`Page`], so results are deterministic and can be paged.
//! Placeholders are numbered as they are bound, so a filter is only added together with its value.

use wg_model::Table;
use wg_model::entity::AutomationRun;
use wg_types::{Id, Timestamp, WgError};

use wg_rows::codec::{Raw, bool_sql, id_sql, text_sql, ts_sql};
use wg_rows::rows::{TableRow, columns};
use wg_rows::sql::{Statement, quote, select_list};

/// The statuses a run holds while it counts against a rule's rate limit (the Python
/// `_rate_limited` query: live, non-dry runs).
const LIVE_STATUSES: [&str; 4] = ["queued", "running", "succeeded", "failed"];

/// One page of a list: at most `limit` rows, after skipping `offset` rows. Python's services call
/// `.limit(n)` with no offset; the offset is there so a caller can page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub limit: i64,
    pub offset: i64,
}

impl Page {
    /// The first `limit` rows.
    pub fn first(limit: i64) -> Page {
        Page { limit, offset: 0 }
    }

    /// `LIMIT ?n OFFSET ?m`, binding both values.
    fn clause(self, params: &mut Vec<Raw>) -> String {
        let limit = bind(params, Raw::Int(self.limit));
        let offset = bind(params, Raw::Int(self.offset));
        format!(" LIMIT {limit} OFFSET {offset}")
    }
}

/// A joined row: a run and the name of its rule (`automation_runs` joined to `automation_rules`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRun {
    pub run: AutomationRun,
    pub rule_name: String,
}

/// Appends `value` to `params` and returns the placeholder that names it (`?N`).
fn bind(params: &mut Vec<Raw>, value: Raw) -> String {
    params.push(value);
    format!("?{}", params.len())
}

/// The rule in `workspace_id` named `name`.
pub fn rule_by_name(workspace_id: Id, name: &str) -> Statement {
    let mut params = Vec::new();
    let ws = bind(&mut params, id_sql(workspace_id));
    let name = bind(&mut params, text_sql(name));
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE \"workspace_id\" = {ws} AND \"name\" = {name}",
            select_list(Table::AutomationRule, None),
            quote(Table::AutomationRule.name())
        ),
        params,
    }
}

/// The rules of `workspace_id`, ordered by name.
pub fn rules(workspace_id: Id) -> Statement {
    let mut params = Vec::new();
    let ws = bind(&mut params, id_sql(workspace_id));
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE \"workspace_id\" = {ws} ORDER BY \"name\"",
            select_list(Table::AutomationRule, None),
            quote(Table::AutomationRule.name())
        ),
        params,
    }
}

/// The enabled rules of every workspace whose `trigger` object has `"type"` equal to
/// `trigger_type` (`"event"` for the dispatcher, `"schedule"` for the cron pass), ordered by
/// workspace then name.
pub fn enabled_rules_of_trigger(trigger_type: &str) -> Statement {
    let mut params = Vec::new();
    let enabled = bind(&mut params, bool_sql(true));
    let trigger = bind(&mut params, text_sql(trigger_type));
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE \"enabled\" = {enabled} \
             AND json_extract(\"trigger\", '$.type') = {trigger} \
             ORDER BY \"workspace_id\", \"name\"",
            select_list(Table::AutomationRule, None),
            quote(Table::AutomationRule.name())
        ),
        params,
    }
}

/// The run of `rule_id` for `trigger_key`. The pair is unique, so this is at most one row; it is
/// the idempotency check a trigger makes before it creates a run.
pub fn run_by_trigger(rule_id: Id, trigger_key: &str) -> Statement {
    let mut params = Vec::new();
    let rule = bind(&mut params, id_sql(rule_id));
    let key = bind(&mut params, text_sql(trigger_key));
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE \"rule_id\" = {rule} AND \"trigger_key\" = {key}",
            select_list(Table::AutomationRun, None),
            quote(Table::AutomationRun.name())
        ),
        params,
    }
}

/// The oldest run, across rules, whose `trigger_key` is `trigger_key` (the first row of the
/// Python lookup by key alone).
pub fn run_by_trigger_key(trigger_key: &str) -> Statement {
    let mut params = Vec::new();
    let key = bind(&mut params, text_sql(trigger_key));
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE \"trigger_key\" = {key} \
             ORDER BY \"created_at\", \"id\" LIMIT 1",
            select_list(Table::AutomationRun, None),
            quote(Table::AutomationRun.name())
        ),
        params,
    }
}

/// The `WHERE` tail shared by [`runs`] and [`runs_with_rule`]: runs of `workspace_id` (`r` is the
/// run's alias), narrowed to one rule and to one status when given.
fn run_filter(
    params: &mut Vec<Raw>,
    workspace_id: Id,
    rule_id: Option<Id>,
    status: Option<&str>,
) -> String {
    let ws = bind(params, id_sql(workspace_id));
    let mut sql = format!(" WHERE r.\"workspace_id\" = {ws}");
    if let Some(rule_id) = rule_id {
        let rule = bind(params, id_sql(rule_id));
        sql.push_str(&format!(" AND r.\"rule_id\" = {rule}"));
    }
    if let Some(status) = status {
        let status = bind(params, text_sql(status));
        sql.push_str(&format!(" AND r.\"status\" = {status}"));
    }
    sql
}

/// The runs of `workspace_id`, newest first, narrowed to `rule_id` and `status` when given. Runs
/// recorded by one decision share its clock reading, so ties are broken by insertion order (newest
/// insertion first), which is the order Python's per-row timestamps give.
pub fn runs(workspace_id: Id, rule_id: Option<Id>, status: Option<&str>, page: Page) -> Statement {
    let mut params = Vec::new();
    let filter = run_filter(&mut params, workspace_id, rule_id, status);
    let limit = page.clause(&mut params);
    Statement {
        sql: format!(
            "SELECT {} FROM {} AS r{filter} ORDER BY r.\"created_at\" DESC, r.rowid DESC{limit}",
            select_list(Table::AutomationRun, Some("r")),
            quote(Table::AutomationRun.name())
        ),
        params,
    }
}

/// The same runs as [`runs`], each joined to its rule for the rule's name. The statement selects
/// the run's columns in schema order, then `rule_name`; decode it with [`decode_rule_run`].
pub fn runs_with_rule(
    workspace_id: Id,
    rule_id: Option<Id>,
    status: Option<&str>,
    page: Page,
) -> Statement {
    let mut params = Vec::new();
    let filter = run_filter(&mut params, workspace_id, rule_id, status);
    let limit = page.clause(&mut params);
    Statement {
        sql: format!(
            "SELECT {}, rule.\"name\" AS \"rule_name\" FROM {} AS r \
             JOIN {} AS rule ON rule.\"id\" = r.\"rule_id\"{filter} \
             ORDER BY r.\"created_at\" DESC, r.rowid DESC{limit}",
            select_list(Table::AutomationRun, Some("r")),
            quote(Table::AutomationRun.name()),
            quote(Table::AutomationRule.name())
        ),
        params,
    }
}

/// Decodes a row of [`runs_with_rule`]: the run's columns, then the rule's name.
pub fn decode_rule_run(values: &[Raw]) -> Result<RuleRun, WgError> {
    let width = columns(Table::AutomationRun).len();
    let run_values = values
        .get(..width)
        .ok_or_else(|| WgError::workgraph("a runs_with_rule row is missing the run columns"))?;
    let run = AutomationRun::from_raw(run_values)?;
    match values.get(width) {
        Some(Raw::Text(rule_name)) => Ok(RuleRun {
            run,
            rule_name: rule_name.clone(),
        }),
        _ => Err(WgError::workgraph("a runs_with_rule row has no rule name")),
    }
}

/// How many of `rule_id`'s live, non-dry runs were created after `since` (the rate-limit window).
/// The statement selects one integer, `COUNT(*)`.
pub fn live_run_count(rule_id: Id, since: Timestamp) -> Statement {
    let mut params = Vec::new();
    let rule = bind(&mut params, id_sql(rule_id));
    let dry = bind(&mut params, bool_sql(false));
    let since = bind(&mut params, ts_sql(since));
    let statuses = LIVE_STATUSES
        .iter()
        .map(|status| format!("'{status}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Statement {
        sql: format!(
            "SELECT COUNT(*) FROM {} WHERE \"rule_id\" = {rule} AND \"dry_run\" = {dry} \
             AND \"status\" IN ({statuses}) AND \"created_at\" > {since}",
            quote(Table::AutomationRun.name())
        ),
        params,
    }
}

/// The notifications of `workspace_id`, newest first; only unread ones when `unread_only`.
pub fn notifications(workspace_id: Id, unread_only: bool, page: Page) -> Statement {
    let mut params = Vec::new();
    let ws = bind(&mut params, id_sql(workspace_id));
    let mut sql = format!(
        "SELECT {} FROM {} WHERE \"workspace_id\" = {ws}",
        select_list(Table::Notification, None),
        quote(Table::Notification.name())
    );
    if unread_only {
        sql.push_str(" AND \"read_at\" IS NULL");
    }
    sql.push_str(" ORDER BY \"created_at\" DESC, \"id\" DESC");
    sql.push_str(&page.clause(&mut params));
    Statement { sql, params }
}

/// The reports of `workspace_id`, newest first.
pub fn reports(workspace_id: Id, page: Page) -> Statement {
    let mut params = Vec::new();
    let ws = bind(&mut params, id_sql(workspace_id));
    let limit = page.clause(&mut params);
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE \"workspace_id\" = {ws} \
             ORDER BY \"created_at\" DESC, \"id\" DESC{limit}",
            select_list(Table::Report, None),
            quote(Table::Report.name())
        ),
        params,
    }
}

/// The finding of `workspace_id` with `fingerprint`. The pair is unique, so this is at most one row.
pub fn finding_by_fingerprint(workspace_id: Id, fingerprint: &str) -> Statement {
    let mut params = Vec::new();
    let ws = bind(&mut params, id_sql(workspace_id));
    let fingerprint = bind(&mut params, text_sql(fingerprint));
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE \"workspace_id\" = {ws} AND \"fingerprint\" = {fingerprint}",
            select_list(Table::Finding, None),
            quote(Table::Finding.name())
        ),
        params,
    }
}
