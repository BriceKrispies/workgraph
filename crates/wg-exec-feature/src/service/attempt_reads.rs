//! Shell: the attempt log route of `api/routes/ops.py`. The other attempt reads of the HTTP routes
//! are the `attempts` service the `attempt` commands share.

use wg_json::Value;
use wg_runtime::App;
use wg_runtime::spec::RequestContext;
use wg_types::{Permission, WgError};
use wg_wire::dto::attempt_dicts;

use crate::service::runs::find_attempt;
use crate::spec::routes::{self, tail_start};

/// The log route of `GET /attempts/{id}/log`: the last `tail` lines of the attempt's agent log,
/// `{available: false}` when the file does not exist. The slice follows Python's `lines[-tail:]`:
/// a zero tail keeps every line, a negative one drops its first lines.
pub fn log(
    app: &mut App,
    context: &RequestContext,
    reference: &str,
    tail: i64,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskRead)?;
    let found = app
        .runtime
        .store
        .read(|session| find_attempt(session, reference))?;
    let path = routes::log_path(
        app.paths.artifacts_dir().as_str(),
        &found.workspace_id.to_string(),
        reference,
    );
    if !path.exists() {
        return Ok(attempt_dicts::log_missing());
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| WgError::workgraph(format!("cannot read {}: {error}", path.display())))?;
    let text = String::from_utf8_lossy(&bytes);
    let lines = wg_json::py_splitlines(&text);
    let total = lines.len();
    let start = tail_start(total, tail);
    let kept: Vec<&str> = lines[start..].to_vec();
    Ok(attempt_dicts::log(
        &kept,
        i64::try_from(total).unwrap_or(i64::MAX),
    ))
}
