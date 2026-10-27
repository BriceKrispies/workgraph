//! The Pure half of the stacks feature: the clap tree of the `stack` group, the human views of its
//! results, and the stack commands as pure functions of the task world (IF-53): `stack create`,
//! `show`, `list`, `add`, `remove`, `readmit`, `integrate`, `verify` and `history`, the `/stacks`
//! routes (`api/routes/stacks.py`) and the stacks service (`core/services/stacks.py`).
//!
//! * [`out`]: `StackOut` assembled from the world, with the integration tips the worktrees report;
//! * [`reads`]: list, show, the stacks containing a task, and the history;
//! * [`writes`]: create, add, remove and readmit as decisions, and the integration and
//!   verification jobs they queue;
//! * [`sql`]: the statement text of the stack readers (SH-07);
//! * [`view`]: the human views of the results.
//!
//! The service loads a [`TaskWorld`](wg_storecodec::task_world_sql::TaskWorld) with the
//! repository rows of every stack of the workspace added, calls one of these and applies the
//! decision it returns. Git (the base commits of a new stack, the tips of the integration
//! worktrees, the branch tips a readmit compares) is the shell's: it asks the functions here for
//! the question and hands the answer back as a value.

pub mod exec;
pub mod out;
pub mod reads;
pub mod sql;
pub mod view;
pub mod writes;

use wg_clispec::Snapshot;
use wg_clispec::tree::{SNAPSHOT_JSON, placeholder};
use wg_model::entity::{Stack, StackItem, Task};
use wg_registry::resolve;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::deps::INACTIVE_STACK_STATUSES;
use wg_types::{ErrorCode, Id, WgError};

/// The `stack` group's clap command, built from the snapshot (`contracts/cli.json`), so its
/// parameters and help match the Python CLI.
#[must_use]
pub fn command() -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "stack"),
        Err(_) => wg_clispec::tree::command("stack"),
    }
}

/// A text member of a validated request body (`StackCreate.parent`, `StackItemAdd.task`); empty
/// when there is none.
#[must_use]
pub fn body_text<'a>(body: &'a serde_json::Value, name: &str) -> &'a str {
    body.get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
}

/// `StackService._active_or_raise`: the parent's stack that has not landed or been abandoned.
pub(crate) fn active_stack<'a>(world: &'a TaskWorld, parent: &Task) -> Result<&'a Stack, WgError> {
    world
        .stacks
        .active_stack_of_parent(parent.id)
        .ok_or_else(|| {
            WgError::new(
                ErrorCode::NotFound,
                format!("{} has no active stack", parent.key),
            )
            .with_detail("parent", parent.key.to_string().into())
        })
}

/// `StackService._latest`: the parent's newest stack, active or not.
pub(crate) fn latest_stack<'a>(world: &'a TaskWorld, parent: &Task) -> Result<&'a Stack, WgError> {
    world
        .stacks
        .stacks
        .values()
        .filter(|stack| stack.parent_task_id == parent.id)
        .max_by_key(|stack| stack.created_at)
        .ok_or_else(|| {
            WgError::new(ErrorCode::NotFound, format!("{} has no stack", parent.key))
                .with_detail("parent", parent.key.to_string().into())
        })
}

/// `StackService._item`: the child's item in the stack.
pub(crate) fn item_of<'a>(
    world: &'a TaskWorld,
    stack: &Stack,
    task: &Task,
) -> Result<&'a StackItem, WgError> {
    world
        .stacks
        .items_of(stack.id)
        .find(|item| item.task_id == task.id)
        .ok_or_else(|| {
            WgError::new(
                ErrorCode::NotFound,
                format!("{} is not in the stack", task.key),
            )
            .with_detail("task", task.key.to_string().into())
        })
}

/// `resolve.task(session, ws_id, ref)`.
pub(crate) fn task_in<'a>(
    world: &'a TaskWorld,
    scope: Id,
    reference: &str,
) -> Result<&'a Task, WgError> {
    resolve::task(&world.tasks, Some(scope), reference)
}

/// Whether a stack has landed or been abandoned (`INACTIVE_STACK_STATUSES`).
pub(crate) fn is_inactive(stack: &Stack) -> bool {
    INACTIVE_STACK_STATUSES.contains(&stack.status)
}

/// A failure of the git runner, as `GitError` is raised by the Python runner.
#[must_use]
pub fn git_failure(err: &wg_git::GitError) -> WgError {
    WgError::new(ErrorCode::GitError, err.to_string())
}

/// A file operation that failed on `path`.
#[must_use]
pub fn io_failure(path: &std::path::Path, err: &std::io::Error) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("{}: {err}", path.display()),
    )
}

/// A file operation that failed, with no path to name.
#[must_use]
pub fn io_error(err: &std::io::Error) -> WgError {
    WgError::new(ErrorCode::WorkgraphError, err.to_string())
}

/// The directory managed worktrees live under, in the platform's own separators.
#[must_use]
pub fn native_root(root: &str) -> std::path::PathBuf {
    std::path::Path::new(root).components().collect()
}

/// `_ctx(job, "stacks")`: the system identity of a job, with its correlation and cause. `fresh` is
/// the identity with a newly drawn correlation, used when the job carries none.
pub fn job_context(
    correlation: Option<wg_types::Correlation>,
    causation: Option<Id>,
    fresh: Result<wg_runtimespec::RequestContext, WgError>,
) -> Result<wg_runtimespec::RequestContext, WgError> {
    let context = match correlation {
        Some(correlation) => wg_runtimespec::RequestContext::system("stacks", correlation)?,
        None => fresh?,
    };
    Ok(context.caused_by(causation))
}
