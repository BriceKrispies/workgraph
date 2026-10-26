//! Integration queue: item detail, queue order, merge sequence, due order and settle detection.
//! Mirrors `load_detail` and `StackIntegrator._next_due` in `integrations/git/stack_integration.py`,
//! and `item_rows`, `_sequence` and `_settled` in `integrations/git/stack_verification.py`.
//! Database reads and job enqueueing stay in the shell; these functions take the rows as values.

use std::collections::HashMap;

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
        let mut detail = ItemDetail::default();
        for (key, value) in parsed {
            match (key.as_str(), value) {
                ("merges", Value::Object(map)) => detail.merges = map,
                ("conflicts", Value::Object(map)) => detail.conflicts = map,
                ("merges" | "conflicts", _) => {}
                (_, value) => {
                    detail.other.insert(key, value);
                }
            }
        }
        detail
    }

    /// The detail as a JSON object with `merges` and `conflicts` always present. The shell
    /// serialises it with sorted keys (`dump_detail`).
    pub fn to_value(&self) -> Value {
        let mut map = self.other.clone();
        map.insert("merges".into(), Value::Object(self.merges.clone()));
        map.insert("conflicts".into(), Value::Object(self.conflicts.clone()));
        Value::Object(map)
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
    let mut sorted: Vec<&QueueItem> = items.iter().collect();
    sorted.sort_by_key(|item| {
        (
            item.merge_order.is_none(),
            item.merge_order.unwrap_or(0),
            item.number,
        )
    });
    sorted
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
    let mut prerequisites: HashMap<&str, Vec<&str>> = HashMap::new();
    for (dependent, prerequisite) in edges {
        if states.contains_key(dependent.as_str()) && states.contains_key(prerequisite.as_str()) {
            prerequisites
                .entry(dependent)
                .or_default()
                .push(prerequisite);
        }
    }
    let mut depths: HashMap<&str, i64> = HashMap::new();
    items
        .iter()
        .filter(|i| i.state == S::Ready && !processed.contains(&i.id))
        .filter(|i| {
            prerequisites.get(i.task_id.as_str()).is_none_or(|ps| {
                ps.iter()
                    .all(|p| matches!(states[p], S::Merged | S::Verified))
            })
        })
        .map(|i| {
            let depth = depth_of(&i.task_id, &prerequisites, &mut depths, 0);
            (
                (depth, priority_rank(i.priority), i.updated_at, i.number),
                i.id.clone(),
            )
        })
        .min()
        .map(|(_, id)| id)
}

/// Dependency depth: 0 without prerequisites, else one more than the deepest prerequisite.
/// `guard` stops a dependency cycle from recursing forever (the Python would overflow).
fn depth_of<'a>(
    task: &'a str,
    prerequisites: &HashMap<&'a str, Vec<&'a str>>,
    depths: &mut HashMap<&'a str, i64>,
    guard: usize,
) -> i64 {
    if let Some(d) = depths.get(task) {
        return *d;
    }
    let depth = match prerequisites.get(task) {
        Some(ps) if !ps.is_empty() && guard < 10_000 => {
            1 + ps
                .iter()
                .map(|p| depth_of(p, prerequisites, depths, guard + 1))
                .max()
                .unwrap_or(0)
        }
        _ => 0,
    };
    depths.insert(task, depth);
    depth
}
