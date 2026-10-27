//! Shell: the attempt reads of the HTTP routes (`AttemptService.list`, `compare`, `manifests` and
//! the attempt log route of `api/routes/ops.py`). Each one checks `task:read` first, as Python does,
//! then reads the store and shapes the reply with the wire DTOs of `wg-wire`.

use wg_json::Value;
use wg_registry::resolve;
use wg_runtime::App;
use wg_runtimespec::RequestContext;
use wg_types::{AttemptStatus, Permission, WgError, parse_id};
use wg_wire::dto::{attempt, attempt_dicts, page};

use crate::service::runs::find_attempt;
use crate::spec::routes::{self, tail_start};

/// The query of `GET /ws/{ws}/attempts`: the filters and the page.
pub struct AttemptFilter<'a> {
    pub task: Option<&'a str>,
    pub statuses: &'a [String],
    pub provider: Option<&'a str>,
    pub limit: i64,
    pub offset: i64,
}

/// `AttemptService.list` for the `GET /ws/{ws}/attempts` route: one page of the workspace's
/// attempts, newest first. A task reference is resolved in the workspace; an unknown status
/// matches nothing, and an empty status list does not narrow the page.
pub fn list(
    app: &mut App,
    context: &RequestContext,
    workspace: &str,
    filter: &AttemptFilter<'_>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskRead)?;
    let workspace_id = app.workspace_id(workspace)?;
    let task_id = match filter.task {
        Some(reference) => Some(app.runtime.store.read(|session| {
            let world = session.task_world(Some(workspace_id), &[])?;
            resolve::task(&world.tasks, Some(workspace_id), reference).map(|task| task.id)
        })?),
        None => None,
    };
    let parsed: Vec<AttemptStatus> = filter
        .statuses
        .iter()
        .filter_map(|status| AttemptStatus::parse(status))
        .collect();
    let matches_nothing = !filter.statuses.is_empty() && parsed.is_empty();
    let (limit, offset) = (filter.limit, filter.offset);
    let found = if matches_nothing {
        wg_store::AttemptPage {
            items: Vec::new(),
            total: 0,
            limit,
            offset,
            next_offset: None,
        }
    } else {
        app.runtime.store.read(|session| {
            session.attempts_page(
                workspace_id,
                task_id,
                &parsed,
                filter.provider,
                limit,
                offset,
            )
        })?
    };
    let items: Vec<Value> = found.items.iter().map(attempt::from_model).collect();
    Ok(page::from_model(
        items,
        found.total,
        found.limit,
        found.offset,
        found.next_offset,
    ))
}

/// `AttemptService.compare` for `GET /attempts/compare`: the side-by-side provenance of two
/// attempts, each named by id (`resolve.attempt`, so an unknown one is `not_found`).
pub fn compare(
    app: &mut App,
    context: &RequestContext,
    first: &str,
    second: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskRead)?;
    let (a, b, manifests_a, manifests_b) = app.runtime.store.read(|session| {
        let a = find_attempt(session, first)?;
        let b = find_attempt(session, second)?;
        let manifests_a = session.manifests_of_attempt(a.id)?;
        let manifests_b = session.manifests_of_attempt(b.id)?;
        Ok((a, b, manifests_a, manifests_b))
    })?;
    Ok(attempt_dicts::compare(&a, &b, &manifests_a, &manifests_b))
}

/// `AttemptService.manifests` for `GET /attempts/{id}/manifests`. Python does not resolve the
/// attempt here: an id that names no attempt, or no UUID, has no manifests and answers an empty
/// list.
pub fn manifests(
    app: &mut App,
    context: &RequestContext,
    reference: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskRead)?;
    let rows = match parse_id(reference) {
        Ok(attempt_id) => app
            .runtime
            .store
            .read(|session| session.manifests_of_attempt(attempt_id))?,
        Err(_) => Vec::new(),
    };
    Ok(Value::List(
        rows.iter().map(attempt_dicts::manifest).collect(),
    ))
}

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
    let bytes = std::fs::read(&path).map_err(|error| {
        WgError::new(
            wg_types::ErrorCode::WorkgraphError,
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
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
