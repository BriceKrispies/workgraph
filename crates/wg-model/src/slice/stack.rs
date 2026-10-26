//! [`StackSlice`]: stacks, their repositories and items; [`StackFlow`]: the flow state of one
//! stack, read from those rows.

use rpds::{RedBlackTreeMapSync, VectorSync};
use wg_types::{Id, StackItemState, StackStatus};

use crate::entity::{Stack, StackItem, StackRepository};
use crate::slice::children;
use crate::write::{Row, RowKey, Write};

/// Loaded rows of the stack family.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StackSlice {
    pub stacks: RedBlackTreeMapSync<Id, Stack>,
    /// Keyed `(stack_id, repository_id)`.
    pub repositories: RedBlackTreeMapSync<(Id, Id), StackRepository>,
    pub items: RedBlackTreeMapSync<Id, StackItem>,
}

impl StackSlice {
    pub fn stack(&self, id: Id) -> Option<&Stack> {
        self.stacks.get(&id)
    }

    /// The stack of a parent task that has not reached a history status (landed or abandoned).
    /// A parent has at most one such stack.
    pub fn active_stack_of_parent(&self, parent_task_id: Id) -> Option<&Stack> {
        self.stacks.values().find(|stack| {
            stack.parent_task_id == parent_task_id
                && !matches!(stack.status, StackStatus::Landed | StackStatus::Abandoned)
        })
    }

    /// The repositories of a stack, in repository-id order.
    pub fn repositories_of(&self, stack_id: Id) -> impl Iterator<Item = &StackRepository> {
        children(&self.repositories, stack_id)
    }

    /// The items of a stack, by item id.
    pub fn items_of(&self, stack_id: Id) -> impl Iterator<Item = &StackItem> {
        self.items
            .values()
            .filter(move |row| row.stack_id == stack_id)
    }

    /// The flow state of a stack, if its row is loaded.
    pub fn flow(&self, stack_id: Id) -> Option<StackFlow> {
        self.stack(stack_id).map(|stack| StackFlow::of(stack, self))
    }

    /// The slice after `write`. Writes to tables this slice does not hold leave it unchanged.
    #[must_use]
    pub fn apply(&self, write: &Write) -> Self {
        match write {
            Write::Insert(row) | Write::Update { row, .. } => self.put(row),
            Write::Delete(key) => self.remove(key),
        }
    }

    /// The slice after every write, in order.
    #[must_use]
    pub fn apply_all<'a>(&self, writes: impl IntoIterator<Item = &'a Write>) -> Self {
        writes
            .into_iter()
            .fold(self.clone(), |slice, write| slice.apply(write))
    }

    fn put(&self, row: &Row) -> Self {
        match row {
            Row::Stack(row) => StackSlice {
                stacks: self.stacks.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::StackRepository(row) => StackSlice {
                repositories: self
                    .repositories
                    .insert((row.stack_id, row.repository_id), row.clone()),
                ..self.clone()
            },
            Row::StackItem(row) => StackSlice {
                items: self.items.insert(row.id, row.clone()),
                ..self.clone()
            },
            _ => self.clone(),
        }
    }

    fn remove(&self, key: &RowKey) -> Self {
        match key {
            RowKey::Stack(id) => StackSlice {
                stacks: self.stacks.remove(id),
                ..self.clone()
            },
            RowKey::StackRepository {
                stack_id,
                repository_id,
            } => StackSlice {
                repositories: self.repositories.remove(&(*stack_id, *repository_id)),
                ..self.clone()
            },
            RowKey::StackItem(id) => StackSlice {
                items: self.items.remove(id),
                ..self.clone()
            },
            _ => self.clone(),
        }
    }
}

/// The state of one stack's flow: its status and the state of each item, in merge order.
///
/// Items are keyed `(merge_order, task_id)`, with items that have no merge order after the
/// ordered ones, so the iteration order is the integration order and identical on every run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackFlow {
    pub stack_id: Id,
    pub parent_task_id: Id,
    pub status: StackStatus,
    pub items: RedBlackTreeMapSync<(i64, Id), (Id, StackItemState)>,
    /// The repositories the stack integrates into (`repository_id` values, in order).
    pub repositories: VectorSync<Id>,
}

impl StackFlow {
    /// The flow state of a stack row, read from the slice's items and repositories.
    pub fn of(stack: &Stack, slice: &StackSlice) -> Self {
        let items = slice
            .items_of(stack.id)
            .fold(RedBlackTreeMapSync::new_sync(), |map, item| {
                map.insert(
                    (item.merge_order.unwrap_or(i64::MAX), item.task_id),
                    (item.id, item.state),
                )
            });
        let repositories = slice
            .repositories_of(stack.id)
            .fold(VectorSync::new_sync(), |list, repo| {
                list.push_back(repo.repository_id)
            });
        StackFlow {
            stack_id: stack.id,
            parent_task_id: stack.parent_task_id,
            status: stack.status,
            items,
            repositories,
        }
    }

    /// Whether every item has reached the state `state`.
    pub fn all_items_in(&self, state: StackItemState) -> bool {
        self.items
            .values()
            .all(|(_, item_state)| *item_state == state)
    }
}
