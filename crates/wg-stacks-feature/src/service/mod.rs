//! The stacks entry points of the CLI: the services behind `stack create, show, list, add, remove,
//! readmit, integrate, verify, history, eject`.
//!
//! Nine of the ten are the application's stack services (`App::stack_*`, IF-53): they check their
//! permission, load the task world, ask git what only git knows and apply the decision. The
//! wrappers here only pass the arguments on. `eject` is checked here: the reason, the parent's active
//! stack, and that the child's merges are on the integration branch (`wg_stacks::ejection`). Its
//! success path, which rebuilds the integration branches without the child, is not ported yet, so
//! an eject that passes its checks fails with that reason.

use wg_json::Value;
use wg_registry::resolve;
use wg_runtime::{App, RequestContext};
use wg_stacks::ejection::{check_ejectable, manual_ejection};
use wg_store::Session;
use wg_types::{ErrorCode, Permission, WgError};

/// Checks a permission against a fresh stamp, before anything is looked up.
fn require(app: &mut App, context: &RequestContext, permission: Permission) -> Result<(), WgError> {
    app.runtime.stamps.stamp(context)?.require(&[permission])
}

/// `StackService.create`.
pub fn create(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
) -> Result<Value, WgError> {
    app.stack_create(context, workspace, parent)
}

/// `StackService.get`: the parent's most recent stack.
pub fn show(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
) -> Result<Value, WgError> {
    app.stack_show(context, workspace, parent)
}

/// `StackService.list`.
pub fn list(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    include_inactive: bool,
) -> Result<Value, WgError> {
    app.stack_list(context, workspace, include_inactive)
}

/// `StackService.add_item`.
pub fn add(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
    task: &str,
) -> Result<Value, WgError> {
    app.stack_add(context, workspace, parent, task)
}

/// `StackService.remove_item`.
pub fn remove(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
    task: &str,
) -> Result<Value, WgError> {
    app.stack_remove(context, workspace, parent, task)
}

/// `StackService.readmit_item`.
pub fn readmit(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
    task: &str,
) -> Result<Value, WgError> {
    app.stack_readmit(context, workspace, parent, task)
}

/// `StackService.integrate`.
pub fn integrate(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
) -> Result<Value, WgError> {
    app.stack_integrate(context, workspace, parent)
}

/// `StackVerifier.request`.
pub fn verify(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
) -> Result<Value, WgError> {
    app.stack_verify(context, workspace, parent)
}

/// `StackService.history`.
pub fn history(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
) -> Result<Value, WgError> {
    app.stack_history(context, workspace, parent)
}

/// `StackVerificationService.eject`, up to the point the Rust rewrite can decide: the permission,
/// the reason (`ValidationFailed`, exit 2), the parent's active stack (`NotFound`) and the child's
/// merged state (`Conflict`, exit 3). A child that passes is not ejected yet, because its rebuild
/// of the integration branches is not ported; the command fails and says so.
pub fn eject(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
    task: &str,
    reason: &str,
) -> Result<Value, WgError> {
    require(app, context, Permission::StackWrite)?;
    manual_ejection(reason)?;
    let scope = app.workspace_or_default(workspace)?;
    let verdict = app.runtime.query(
        |session| eject_verdict(session, scope, parent, task),
        |verdict| Ok(verdict.clone()),
    )?;
    match verdict.state {
        Some(state) => check_ejectable(
            &verdict.task_key,
            &verdict.parent_key,
            state,
            verdict.has_merges,
        )?,
        // An item that is not in the stack has no merges on its branch either.
        None => {
            return Err(WgError::new(
                ErrorCode::Conflict,
                format!(
                    "{} is not merged on {}'s stack; only a merged child can be ejected",
                    verdict.task_key, verdict.parent_key
                ),
            )
            .with_detail("task", verdict.task_key.as_str().into()));
        }
    }
    Err(WgError::new(
        ErrorCode::WorkgraphError,
        format!(
            "{} is merged on {}'s stack, and ejecting it needs the rebuild of the integration branches, which is not ported to the Rust rewrite yet",
            verdict.task_key, verdict.parent_key
        ),
    ))
}

/// What an eject needs to know: the keys, and the child's item state and whether it records merges.
#[derive(Clone, Debug)]
struct EjectVerdict {
    parent_key: String,
    task_key: String,
    state: Option<wg_types::StackItemState>,
    has_merges: bool,
}

/// Loads the parent's active stack and the child's item in it.
fn eject_verdict(
    session: &mut Session<'_>,
    scope: wg_types::Id,
    parent: &str,
    task: &str,
) -> Result<EjectVerdict, WgError> {
    let world = session.task_world(Some(scope), &[])?;
    let parent_task = resolve::task(&world.tasks, Some(scope), parent)?;
    let child = resolve::task(&world.tasks, Some(scope), task)?;
    let stack = world
        .stacks
        .active_stack_of_parent(parent_task.id)
        .ok_or_else(|| {
            WgError::new(
                ErrorCode::NotFound,
                format!("{} has no active stack", parent_task.key),
            )
            .with_detail("parent", parent_task.key.to_string().into())
        })?;
    let item = world
        .stacks
        .items_of(stack.id)
        .find(|item| item.task_id == child.id);
    let has_merges = item
        .and_then(|item| item.detail.as_deref())
        .and_then(|detail| serde_json::from_str::<serde_json::Value>(detail).ok())
        .and_then(|detail| {
            detail
                .get("merges")
                .and_then(serde_json::Value::as_object)
                .map(|merges| !merges.is_empty())
        })
        .unwrap_or(false);
    Ok(EjectVerdict {
        parent_key: parent_task.key.to_string(),
        task_key: child.key.to_string(),
        state: item.map(|item| item.state),
        has_merges,
    })
}
