//! The stored reports of a workspace (`GET /reports`, H112): `list_reports`, the 20 newest rows of
//! the `reports` table, newest first.

use wg_json::Value;
use wg_runtime::{App, RequestContext};
use wg_storecodec::{Raw, sql::Statement};
use wg_types::{Permission, Timestamp, WgError};
use wg_wire::dto::analytics_report_entry;

/// How many reports `list_reports` returns (Python's default `limit`).
const LIMIT: u32 = 20;

/// `list_reports(app, workspace)`: the stored reports, each as `ReportOut`. The permission is
/// `ANALYTICS_READ`, checked before the workspace is resolved, as the route does.
pub fn list_reports(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::AnalyticsRead)?;
    let ws = app.workspace_or_default(workspace)?;
    let statement = Statement {
        sql: format!(
            "SELECT \"id\", \"kind\", \"title\", \"narrative_source\", \"created_at\", \
             \"content_markdown\" FROM \"reports\" WHERE \"workspace_id\" = ?1 \
             ORDER BY \"created_at\" DESC LIMIT {LIMIT}"
        ),
        params: vec![Raw::Text(ws.to_string())],
    };
    let rows = app.runtime.store.read(|session| session.rows(&statement))?;
    rows.iter()
        .map(|row| report(row))
        .collect::<Result<Vec<Value>, WgError>>()
        .map(Value::List)
}

/// One stored report row as `ReportOut`.
fn report(row: &[Raw]) -> Result<Value, WgError> {
    let text = |index: usize| match row.get(index) {
        Some(Raw::Text(text)) => text.clone(),
        _ => String::new(),
    };
    let created_at = text(4);
    let naive = created_at.replace("+00:00", "");
    let at =
        Timestamp::parse_iso(&format!("{naive}Z")).or_else(|_| Timestamp::parse_iso(&naive))?;
    Ok(analytics_report_entry::from_parts(
        &text(0),
        &text(1),
        &text(2),
        &text(3),
        at,
        &text(5),
    ))
}
