//! The stack reads: `StackService.list`, `get`, `containing` and `history`, and the question each
//! asks of git (the tips of the integration worktrees).

use serde_json::Value as Json;
use wg_json::Value;
use wg_model::entity::Stack;
use wg_storecodec::EventRecord;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_types::{Id, WgError, parse_id};

use super::out::{Tips, stack_list_out, stack_out};
use super::{is_inactive, latest_stack, task_in};
use crate::events::event_out;

/// One integration worktree whose tip a stack reply reports: `StackIntegrator.worktree_path`,
/// relative to the worktrees directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TipTarget {
    pub stack_id: Id,
    pub repository: String,
    /// `stacks/<stack id>/<directory name of the repository>`.
    pub directory: String,
}

/// `StackIntegrator.worktree_path` relative to the worktrees directory.
#[must_use]
pub fn tip_directory(stack_id: Id, repository: &str) -> String {
    format!(
        "stacks/{stack_id}/{}",
        wg_stacks::merge::directory_name(repository)
    )
}

/// The worktrees of `stacks`, one per repository, in stack then repository order.
fn targets_of<'a>(
    world: &TaskWorld,
    stacks: impl IntoIterator<Item = &'a Stack>,
) -> Vec<TipTarget> {
    stacks
        .into_iter()
        .flat_map(|stack| {
            world
                .stacks
                .repositories_of(stack.id)
                .filter_map(|row| world.tasks.registry.repository(row.repository_id))
                .map(|repository| TipTarget {
                    stack_id: stack.id,
                    repository: repository.name.clone(),
                    directory: tip_directory(stack.id, &repository.name),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The stacks of the workspace `scope`, newest first; landed and abandoned ones only with
/// `include_inactive`.
fn listed(world: &TaskWorld, scope: Id, include_inactive: bool) -> Vec<&Stack> {
    let mut rows: Vec<&Stack> = world
        .stacks
        .stacks
        .values()
        .filter(|stack| stack.workspace_id == scope && (include_inactive || !is_inactive(stack)))
        .collect();
    rows.sort_by_key(|stack| std::cmp::Reverse(stack.created_at));
    rows
}

/// The worktrees `list` reports tips for.
pub fn list_targets(world: &TaskWorld, scope: Id, include_inactive: bool) -> Vec<TipTarget> {
    targets_of(world, listed(world, scope, include_inactive))
}

/// `StackService.list`.
pub fn list(
    world: &TaskWorld,
    scope: Id,
    include_inactive: bool,
    tips: &Tips,
) -> Result<Value, WgError> {
    stack_list_out(world, listed(world, scope, include_inactive), tips)
}

/// The worktrees `get` reports tips for: those of the parent's newest stack.
pub fn show_targets(
    world: &TaskWorld,
    scope: Id,
    parent_ref: &str,
) -> Result<Vec<TipTarget>, WgError> {
    let parent = task_in(world, scope, parent_ref)?;
    Ok(targets_of(world, [latest_stack(world, parent)?]))
}

/// `StackService.get`: the parent's newest stack.
pub fn show(world: &TaskWorld, scope: Id, parent_ref: &str, tips: &Tips) -> Result<Value, WgError> {
    let parent = task_in(world, scope, parent_ref)?;
    stack_out(world, latest_stack(world, parent)?, tips)
}

/// `StackService.containing`: every stack the task is an item of, newest first, without tips.
pub fn containing(world: &TaskWorld, scope: Id, task_ref: &str) -> Result<Value, WgError> {
    let child = task_in(world, scope, task_ref)?;
    let mut rows: Vec<&Stack> = world
        .stacks
        .stacks
        .values()
        .filter(|stack| {
            world
                .stacks
                .items_of(stack.id)
                .any(|item| item.task_id == child.id)
        })
        .collect();
    rows.sort_by_key(|stack| std::cmp::Reverse(stack.created_at));
    stack_list_out(world, rows, &Tips::new())
}

/// `StackService.history`, first step: the id of the parent's newest stack, whose events are the
/// history.
pub fn history_stack(world: &TaskWorld, scope: Id, parent_ref: &str) -> Result<Id, WgError> {
    let parent = task_in(world, scope, parent_ref)?;
    Ok(latest_stack(world, parent)?.id)
}

/// The verification runs a stack's events name: `run_id`, or `verification_run_id` for
/// ejections, whichever are ids.
pub fn run_ids(events: &[EventRecord]) -> Vec<Id> {
    let found: std::collections::BTreeSet<Id> = events
        .iter()
        .flat_map(|event| {
            ["run_id", "verification_run_id"]
                .into_iter()
                .filter_map(|key| event.data.get(key).and_then(Json::as_str))
                .filter_map(|text| parse_id(text).ok())
                .collect::<Vec<_>>()
        })
        .collect();
    found.into_iter().collect()
}

/// `StackService.history`, last step: the stack's events and the events of the runs they name,
/// in `seq` order.
pub fn history_out(events: Vec<EventRecord>, runs: Vec<EventRecord>) -> Value {
    let mut merged: Vec<EventRecord> = events.into_iter().chain(runs).collect();
    merged.sort_by_key(|event| event.seq);
    Value::List(merged.iter().map(event_out).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worktree_directory_is_under_the_stack_and_sanitised() {
        let stack = wg_model::derive_id(&[1; 32], 0);
        assert_eq!(
            tip_directory(stack, "my repo"),
            format!("stacks/{stack}/my_repo")
        );
    }
}
