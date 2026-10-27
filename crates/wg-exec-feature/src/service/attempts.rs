//! The attempt reads of the `attempt` command group (`core/services/attempts.py`): `list`, `get`,
//! `manifests` and `compare`. Each checks `task:read`, resolves its references as the service
//! does, and answers with the wire shapes the CLI prints.

use wg_json::Value;
use wg_runtime::{App, RequestContext};
use wg_store::AttemptPage;
use wg_types::{AttemptStatus, Permission, WgError, parse_id};
use wg_wire::dto::{attempt, attempt_dicts, page};

use super::resolve::{find_attempt, find_task};

/// The options of `attempt list` that narrow the page.
pub struct ListQuery<'a> {
    pub workspace: Option<&'a str>,
    pub task: Option<&'a str>,
    pub statuses: &'a [String],
    pub provider: Option<&'a str>,
    pub limit: i64,
    pub offset: i64,
}

/// `AttemptService.list`: one page of the workspace's attempts, newest first. A status that is
/// not a known one matches no row; an empty filter does not narrow.
pub fn attempt_list(
    app: &mut App,
    context: &RequestContext,
    query: &ListQuery<'_>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskRead)?;
    let workspace = app.workspace_or_default(query.workspace)?;
    let task = query.task.filter(|text| !text.is_empty());
    let provider = query.provider.filter(|text| !text.is_empty());
    let task_id = match task {
        Some(reference) => Some(
            app.runtime
                .store
                .read(|session| find_task(session, Some(workspace), reference))?
                .id,
        ),
        None => None,
    };
    let statuses: Vec<AttemptStatus> = query
        .statuses
        .iter()
        .filter_map(|status| AttemptStatus::parse(status))
        .collect();
    let matches_nothing = !query.statuses.is_empty() && statuses.is_empty();
    let page = if matches_nothing {
        AttemptPage {
            items: Vec::new(),
            total: 0,
            limit: query.limit,
            offset: query.offset,
            next_offset: None,
        }
    } else {
        app.runtime.store.read(|session| {
            session.attempts_page(
                workspace,
                task_id,
                &statuses,
                provider,
                query.limit,
                query.offset,
            )
        })?
    };
    let items: Vec<_> = page.items.iter().map(attempt::from_model).collect();
    Ok(page::from_model(
        items,
        page.total,
        page.limit,
        page.offset,
        page.next_offset,
    ))
}

/// `AttemptService.get`: one attempt by UUID, as `AttemptOut`.
pub fn attempt_get(
    app: &mut App,
    context: &RequestContext,
    reference: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskRead)?;
    let found = app
        .runtime
        .store
        .read(|session| find_attempt(session, reference))?;
    Ok(attempt::from_model(&found))
}

/// `AttemptService.manifests`: the revision manifests of an attempt, oldest first. The reference
/// is not resolved: a value that is not a UUID, or no attempt, has no manifests.
pub fn attempt_manifests(
    app: &mut App,
    context: &RequestContext,
    reference: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskRead)?;
    let rows = match parse_id(reference) {
        Ok(id) => app
            .runtime
            .store
            .read(|session| session.manifests_of_attempt(id))?,
        Err(_) => Vec::new(),
    };
    Ok(attempt_dicts::manifests(&rows))
}

/// `AttemptService.compare`: the provenance of two attempts side by side.
pub fn attempt_compare(
    app: &mut App,
    context: &RequestContext,
    first: &str,
    second: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskRead)?;
    let (a, b) = app.runtime.store.read(|session| {
        let a = find_attempt(session, first)?;
        let b = find_attempt(session, second)?;
        Ok((a, b))
    })?;
    let (manifests_a, manifests_b) = app.runtime.store.read(|session| {
        Ok::<_, WgError>((
            session.manifests_of_attempt(a.id)?,
            session.manifests_of_attempt(b.id)?,
        ))
    })?;
    Ok(attempt_dicts::compare(&a, &b, &manifests_a, &manifests_b))
}
