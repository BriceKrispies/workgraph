//! The stack executor as decisions: what the `stack_integrate` and `stack_verify` jobs write
//! (`integrations/git/stack_integration.py`, `stack_verification.py`, `stack_landing.py`).
//!
//! Each job interleaves short transactions with git and process runs. A transaction is one
//! function here over the task world the shell loaded in it, returning the decision to apply;
//! the shell owns the git commands, the check processes and the order of the steps.
//!
//! * [`chain`]: the steps one transaction takes, applied one after another;
//! * [`worktree`]: integration targets and the managed worktree rows;
//! * [`integrate`]: the merge queue;
//! * [`verify`]: verification of the tip, bisection, ejection and the rebuild;
//! * [`land`]: moving the default branches and accepting the stack;
//! * [`run`]: the verification run a stack verification starts;
//! * [`text`]: the JSON text of item details and landing requests.

pub mod chain;
pub mod check;
pub mod integrate;
pub mod land;
pub mod run;
pub mod text;
pub mod verify;
pub mod worktree;

use rpds::VectorSync;
use serde_json::{Map, Value};
use wg_model::event::StrMap;
use wg_model::slice::StackSlice;
use wg_model::{Task, TaskSlice};
use wg_rows::task_world_sql::TaskWorld;
use wg_stacks::bisect::StackRow;
use wg_stacks::queue::QueueItem;
use wg_stacks::verify::StackRepoBase;
use wg_types::Id;

pub use chain::Chain;

/// The repositories of a stack with their base commits, by repository name (`_repos`).
#[must_use]
pub fn repos_of(world: &TaskWorld, stack_id: Id) -> Vec<StackRepoBase> {
    let mut repos: Vec<StackRepoBase> = world
        .stacks
        .repositories_of(stack_id)
        .filter_map(|link| {
            world
                .tasks
                .registry
                .repository(link.repository_id)
                .map(|repository| StackRepoBase {
                    repository_id: link.repository_id.to_string(),
                    name: repository.name.clone(),
                    base_sha: link.base_sha.clone(),
                })
        })
        .collect();
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    repos
}

/// The items of a stack with their tasks, in `item_rows` order: merge order (unordered last),
/// then task number.
#[must_use]
pub fn item_rows(
    stacks: &StackSlice,
    tasks: &TaskSlice,
    stack_id: Id,
) -> Vec<(wg_model::entity::StackItem, Task)> {
    let mut rows: Vec<(wg_model::entity::StackItem, Task)> = stacks
        .items_of(stack_id)
        .filter_map(|item| {
            tasks
                .task(item.task_id)
                .map(|task| (item.clone(), task.clone()))
        })
        .collect();
    rows.sort_by_key(|(item, task)| (item.merge_order.is_none(), item.merge_order, task.number));
    rows
}

/// The same rows as the bisection reads them.
#[must_use]
pub fn stack_rows(stacks: &StackSlice, tasks: &TaskSlice, stack_id: Id) -> Vec<StackRow> {
    item_rows(stacks, tasks, stack_id)
        .into_iter()
        .map(|(item, task)| StackRow {
            item_id: item.id.to_string(),
            task_id: task.id.to_string(),
            title: task.title.clone(),
            item: QueueItem {
                key: task.key.to_string(),
                number: task.number,
                merge_order: item.merge_order,
                state: item.state,
                detail: item.detail.clone(),
            },
        })
        .collect()
}

/// A JSON object of strings, in key order.
#[must_use]
pub fn str_map<'a>(pairs: impl IntoIterator<Item = (&'a String, &'a String)>) -> StrMap {
    StrMap(
        pairs
            .into_iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<VectorSync<_>>(),
    )
}

/// A list of strings for an event payload.
#[must_use]
pub fn str_list(items: &[String]) -> VectorSync<String> {
    items.iter().cloned().collect()
}

/// The result object of a finished job: the members of the value a flow returned.
#[must_use]
pub fn job_result(value: &Value) -> Option<Map<String, Value>> {
    value.as_object().cloned()
}
