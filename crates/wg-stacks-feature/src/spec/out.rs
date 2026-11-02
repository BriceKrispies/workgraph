//! `StackOut` for a stack of the world (`stack_out` in `core/services/stacks.py`).

use std::collections::BTreeMap;

use wg_json::Value;
use wg_model::entity::{Stack, StackItem, StackRepository, Task};
use wg_rows::task_world_sql::TaskWorld;
use wg_types::{Id, WgError};
use wg_wire::dto::stack;

/// The integration tip each prepared worktree reported, by stack and repository name. A
/// repository with no entry has no worktree yet.
pub type Tips = BTreeMap<(Id, String), String>;

/// The head an item's newest succeeded attempt recorded in one repository, with the attempt's
/// branch and the commit that branch points at now (`None` when the branch is not read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemHead {
    pub task_id: Id,
    pub repository: String,
    pub head: String,
    pub branch: Option<String>,
    pub tip: Option<String>,
}

/// The task ids of a stack's items, in the order `stack_out` lists them.
pub fn item_task_ids(world: &TaskWorld, stack: &Stack) -> Vec<Id> {
    items(world, stack)
        .into_iter()
        .map(|(_, task)| task.id)
        .collect()
}

/// Adds `recorded_heads` to each item of a `stack_out` reply. `task_order` is the item order of
/// that reply (see [`item_task_ids`]); an item with no recorded head gets an empty list.
pub fn with_item_heads(value: Value, task_order: &[Id], heads: &[ItemHead]) -> Value {
    let Value::Dict(fields) = value else {
        return value;
    };
    let fields = fields
        .into_iter()
        .map(|(key, field)| {
            let is_items = key.to_text().as_deref() == Some("items");
            match field {
                Value::List(rows) if is_items => {
                    let rows = rows
                        .into_iter()
                        .zip(task_order)
                        .map(|(row, task_id)| match row {
                            Value::Dict(mut item) => {
                                item.push((
                                    "recorded_heads".into(),
                                    recorded_heads(task_id, heads),
                                ));
                                Value::Dict(item)
                            }
                            other => other,
                        })
                        .collect();
                    (key, Value::List(rows))
                }
                other => (key, other),
            }
        })
        .collect();
    Value::Dict(fields)
}

/// The `recorded_heads` list of one item.
fn recorded_heads(task_id: &Id, heads: &[ItemHead]) -> Value {
    Value::List(
        heads
            .iter()
            .filter(|head| &head.task_id == task_id)
            .map(|head| {
                Value::Dict(vec![
                    (
                        "repository".into(),
                        Value::Str(head.repository.as_str().into()),
                    ),
                    ("head".into(), Value::Str(head.head.as_str().into())),
                    ("branch".into(), optional_text(head.branch.as_deref())),
                    ("tip".into(), optional_text(head.tip.as_deref())),
                ])
            })
            .collect(),
    )
}

fn optional_text(text: Option<&str>) -> Value {
    text.map_or(Value::Null, |text| Value::Str(text.into()))
}

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
    let parent = world
        .tasks
        .task(stack_row.parent_task_id)
        .ok_or_else(|| WgError::workgraph("the stack's parent task vanished"))?;
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
