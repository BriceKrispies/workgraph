//! Integration queue: item detail, queue order, merge sequence, due order and settle detection.
//! Mirrors `load_detail` and `StackIntegrator._next_due` in `integrations/git/stack_integration.py`,
//! and `item_rows`, `_sequence` and `_settled` in `integrations/git/stack_verification.py`.
//! Database reads and job enqueueing stay in the shell; these functions take the rows as values.

use std::collections::{BTreeMap, HashMap};

use rpds::HashTrieMap;
use serde_json::{Map, Value};
use wg_types::{StackItemState, TaskPriority};

use crate::item::is_on_branch;

use StackItemState as S;

/// An item's parsed detail: the `merges` and `conflicts` maps plus every other key unchanged.
///
/// Free text set by an operator carries no merges, so unparseable or non-object text reads as empty.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemDetail {
    /// Keys other than `merges` and `conflicts`, in their parsed order.
    pub other: Map<String, Value>,
    /// Repository name to `{head, merge_commit}`.
    pub merges: Map<String, Value>,
    /// Repository name to the open conflict.
    pub conflicts: Map<String, Value>,
}

impl ItemDetail {
    /// `load_detail`: parses stored detail text.
    pub fn load(text: Option<&str>) -> ItemDetail {
        let parsed = match text
            .filter(|t| !t.is_empty())
            .map(serde_json::from_str::<Value>)
        {
            Some(Ok(Value::Object(map))) => map,
            _ => Map::new(),
        };
        let object_or_empty = |key: &str| match parsed.get(key) {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        let merges = object_or_empty("merges");
        let conflicts = object_or_empty("conflicts");
        let other = parsed
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "merges" | "conflicts"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        ItemDetail {
            other,
            merges,
            conflicts,
        }
    }

    /// The detail as a JSON object with `merges` and `conflicts` always present. The shell
    /// serialises it with sorted keys (`dump_detail`).
    pub fn to_value(&self) -> Value {
        // Keys already in `other` keep their place, as an insert would leave them; the two
        // sections not in `other` follow in that order.
        let section = |key: &str| {
            Value::Object(if key == "merges" {
                self.merges.clone()
            } else {
                self.conflicts.clone()
            })
        };
        let entries = self
            .other
            .iter()
            .map(|(key, value)| match key.as_str() {
                "merges" | "conflicts" => (key.clone(), section(key.as_str())),
                _ => (key.clone(), value.clone()),
            })
            .chain(
                ["merges", "conflicts"]
                    .into_iter()
                    .filter(|key| !self.other.contains_key(*key))
                    .map(|key| (key.to_owned(), section(key))),
            );
        Value::Object(entries.collect())
    }
}

/// One stack item as the queue sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueItem {
    pub key: String,
    pub number: i64,
    pub merge_order: Option<i64>,
    pub state: StackItemState,
    /// Stored detail text.
    pub detail: Option<String>,
}

/// `item_rows` order: `ORDER BY merge_order IS NULL, merge_order, Task.number`.
pub fn ordered_items(items: &[QueueItem]) -> Vec<&QueueItem> {
    // The position is part of the key, so equal sort keys keep their input order (a stable sort).
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            (
                (
                    item.merge_order.is_none(),
                    item.merge_order.unwrap_or(0),
                    item.number,
                    index,
                ),
                item,
            )
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

/// `_sequence`: keys of the items whose merges are on the integration branch, in merge order.
pub fn merge_sequence(items: &[QueueItem]) -> Vec<String> {
    ordered_items(items)
        .into_iter()
        .filter(|item| {
            is_on_branch(item.state) && !ItemDetail::load(item.detail.as_deref()).merges.is_empty()
        })
        .map(|item| item.key.clone())
        .collect()
}

/// `_settled`: no item is waiting to merge, and at least one has merged.
pub fn is_settled(states: &[StackItemState]) -> bool {
    !states.iter().any(|s| matches!(s, S::Pending | S::Ready))
        && states.iter().any(|s| matches!(s, S::Merged | S::Verified))
}

/// `PRIORITY_RANK`: lower ranks integrate first.
pub fn priority_rank(priority: TaskPriority) -> i64 {
    match priority {
        TaskPriority::Critical => 0,
        TaskPriority::High => 1,
        TaskPriority::Medium => 2,
        TaskPriority::Low => 3,
    }
}

/// One stack item for [`next_due`].
#[derive(Debug, Clone, PartialEq)]
pub struct DueItem {
    /// Item id.
    pub id: String,
    /// Task id.
    pub task_id: String,
    pub state: StackItemState,
    pub priority: TaskPriority,
    /// Item `updated_at`, as a value that orders chronologically (microseconds since the epoch).
    pub updated_at: i64,
    pub number: i64,
}

/// `_next_due`: the id of the next ready item whose in-stack prerequisites are merged, by
/// dependency depth, priority, readiness time, then task number.
///
/// `edges` are (dependent task id, prerequisite task id) pairs; pairs that leave the stack are
/// ignored. `processed` holds item ids already handled in this run.
pub fn next_due(
    items: &[DueItem],
    edges: &[(String, String)],
    processed: &[String],
) -> Option<String> {
    let states: HashMap<&str, StackItemState> = items
        .iter()
        .map(|i| (i.task_id.as_str(), i.state))
        .collect();
    let in_stack: Vec<(&str, &str)> = edges
        .iter()
        .filter(|(dependent, prerequisite)| {
            states.contains_key(dependent.as_str()) && states.contains_key(prerequisite.as_str())
        })
        .map(|(dependent, prerequisite)| (dependent.as_str(), prerequisite.as_str()))
        .collect();
    let due = items
        .iter()
        .filter(|i| i.state == S::Ready && !processed.contains(&i.id))
        .filter(|i| {
            prerequisites_of(&in_stack, &i.task_id)
                .iter()
                .all(|p| matches!(states[p], S::Merged | S::Verified))
        });
    // The depth memo is threaded through the fold, so each task's depth is computed once.
    let (_, best) = due.fold(
        (HashTrieMap::new(), None),
        |(memo, best): (HashTrieMap<&str, i64>, Option<DueKey>), i| {
            let (depth, memo) = depth_of(&i.task_id, &in_stack, memo, 0);
            let candidate = (
                (depth, priority_rank(i.priority), i.updated_at, i.number),
                i.id.clone(),
            );
            let best = match best {
                Some(current) if current <= candidate => Some(current),
                _ => Some(candidate),
            };
            (memo, best)
        },
    );
    best.map(|(_, id)| id)
}

/// The sort key of a due candidate, with its item id: the smallest one wins.
type DueKey = ((i64, i64, i64, i64), String);

/// The prerequisites of `task` among the in-stack edges, in edge order.
fn prerequisites_of<'a>(edges: &[(&'a str, &'a str)], task: &str) -> Vec<&'a str> {
    edges
        .iter()
        .filter(|(dependent, _)| *dependent == task)
        .map(|(_, prerequisite)| *prerequisite)
        .collect()
}

/// Dependency depth: 0 without prerequisites, else one more than the deepest prerequisite.
/// `guard` stops a dependency cycle from recursing forever (the Python would overflow). Returns
/// the depth with the memo extended by every depth computed on the way.
fn depth_of<'a>(
    task: &'a str,
    edges: &[(&'a str, &'a str)],
    memo: HashTrieMap<&'a str, i64>,
    guard: usize,
) -> (i64, HashTrieMap<&'a str, i64>) {
    if let Some(depth) = memo.get(task) {
        return (*depth, memo);
    }
    let prerequisites = prerequisites_of(edges, task);
    let (depth, memo) = if !prerequisites.is_empty() && guard < 10_000 {
        let (deepest, memo) = prerequisites.iter().fold(
            (None, memo),
            |(deepest, memo): (Option<i64>, HashTrieMap<&'a str, i64>), prerequisite| {
                let (depth, memo) = depth_of(prerequisite, edges, memo, guard + 1);
                (Some(deepest.map_or(depth, |d| d.max(depth))), memo)
            },
        );
        (1 + deepest.unwrap_or(0), memo)
    } else {
        (0, memo)
    };
    (depth, memo.insert(task, depth))
}
