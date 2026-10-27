//! `StackOut` for a stack of the world (`stack_out` in `core/services/stacks.py`).

use std::collections::BTreeMap;

use wg_json::Value;
use wg_model::entity::{Stack, StackItem, StackRepository, Task};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_types::{ErrorCode, Id, WgError};
use wg_wire::dto::stack;

/// The integration tip each prepared worktree reported, by stack and repository name. A
/// repository with no entry has no worktree yet.
pub type Tips = BTreeMap<(Id, String), String>;

/// The repository rows of a stack with their repository's name, ordered by name.
fn repositories<'a>(world: &'a TaskWorld, stack: &Stack) -> Vec<(&'a StackRepository, &'a str)> {
    let mut rows: Vec<(&StackRepository, &str)> = world
        .stacks
        .repositories_of(stack.id)
        .filter_map(|row| {
            world
                .tasks
                .registry
                .repository(row.repository_id)
                .map(|repository| (row, repository.name.as_str()))
        })
        .collect();
    rows.sort_by(|a, b| a.1.cmp(b.1));
    rows
}

/// The items of a stack with their task, ordered by merge order (unordered items last), then by
/// task number.
fn items<'a>(world: &'a TaskWorld, stack: &Stack) -> Vec<(&'a StackItem, &'a Task)> {
    let mut rows: Vec<(&StackItem, &Task)> = world
        .stacks
        .items_of(stack.id)
        .filter_map(|item| world.tasks.task(item.task_id).map(|task| (item, task)))
        .collect();
    rows.sort_by_key(|(item, task)| (item.merge_order.is_none(), item.merge_order, task.number));
    rows
}

/// `StackOut` of `stack`; `tips` fills each repository's `integration_tip` (`_with_tips`), and an
/// empty map leaves them null (`stack_out`).
pub fn stack_out(world: &TaskWorld, stack_row: &Stack, tips: &Tips) -> Result<Value, WgError> {
    let parent = world.tasks.task(stack_row.parent_task_id).ok_or_else(|| {
        WgError::new(
            ErrorCode::WorkgraphError,
            "the stack's parent task vanished",
        )
    })?;
    let repositories = repositories(world, stack_row);
    Ok(stack::with_tips(
        stack_row,
        parent,
        repositories.iter().map(|(row, name)| {
            (
                *row,
                *name,
                tips.get(&(stack_row.id, (*name).to_owned()))
                    .map(String::as_str),
            )
        }),
        items(world, stack_row),
    ))
}

/// A list of `StackOut`.
pub fn stack_list_out<'a>(
    world: &TaskWorld,
    stacks: impl IntoIterator<Item = &'a Stack>,
    tips: &Tips,
) -> Result<Value, WgError> {
    stacks
        .into_iter()
        .map(|row| stack_out(world, row, tips))
        .collect::<Result<Vec<_>, _>>()
        .map(Value::List)
}
