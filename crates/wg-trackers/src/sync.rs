//! Sync decisions: the pure parts of reconciling tracker snapshots with Workgraph tasks.
//!
//! The shell reads the task rows, the repository and dependency rows, and the ancestry and
//! reachability facts, and passes them in as plain values. Every function here decides; none
//! reads or writes a database.

use std::collections::HashMap;

use rpds::{HashTrieSet, Vector};
use serde_json::{Value, json};
use wg_types::TaskPriority;

use crate::canonical::canonical_json;
use crate::mapping::TrackerSnapshot;

/// The project key of an issue key: everything before the last hyphen.
#[must_use]
pub fn project_of(key: &str) -> &str {
    key.rsplit_once('-').map_or(key, |(project, _)| project)
}

fn referenced_keys(snap: &TrackerSnapshot) -> impl Iterator<Item = &str> {
    snap.parent_key
        .iter()
        .chain(snap.blocked_by_keys.iter())
        .map(String::as_str)
}

/// The snapshots in dependency order: parents and blockers before the issues that reference
/// them. Cycles keep their input order. When two snapshots share a key, the later one is the
/// one a reference resolves to, and the earlier one still keeps its own place in the input.
#[must_use]
pub fn dependency_order(snaps: &[TrackerSnapshot]) -> Vec<&TrackerSnapshot> {
    // Later snapshots overwrite earlier ones with the same key, as `HashMap::insert` does.
    let by_key: HashMap<&str, usize> = snaps
        .iter()
        .enumerate()
        .map(|(index, snap)| (snap.key.as_str(), index))
        .collect();
    let (_, order) = (0..snaps.len()).fold((HashTrieSet::new(), Vector::new()), |state, index| {
        visit(index, snaps, &by_key, state)
    });
    order.iter().map(|&index| &snaps[index]).collect()
}

/// Visits one snapshot: its referenced snapshots first, then itself. The state holds the keys
/// already visited and the order so far; both are persistent, so each step returns a new state.
fn visit<'a>(
    index: usize,
    snaps: &'a [TrackerSnapshot],
    by_key: &HashMap<&str, usize>,
    state: (HashTrieSet<&'a str>, Vector<usize>),
) -> (HashTrieSet<&'a str>, Vector<usize>) {
    let snap = &snaps[index];
    let key = snap.key.as_str();
    let (visited, order) = state;
    if visited.contains(&key) {
        return (visited, order);
    }
    let visited = visited.insert(key);
    let (visited, order) = referenced_keys(snap)
        .filter_map(|reference| by_key.get(reference).copied())
        .fold((visited, order), |state, target| {
            visit(target, snaps, by_key, state)
        });
    (visited, order.push_back(index))
}

/// The Workgraph rows a snapshot's links point at, resolved by the shell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncLinks {
    /// Registered repository ids, in first-seen order without duplicates.
    pub repositories: Vec<String>,
    /// The parent task id, when the parent is synced.
    pub parent: Option<String>,
    /// Blocker task ids, sorted.
    pub blockers: Vec<String>,
}

/// The change digest of a snapshot and its links: SHA-256 of the canonical JSON of
/// `{"snapshot": ..., "links": ...}`, byte-identical to the Python `_snapshot_hash`. The raw
/// issue JSON is left out; `remote_updated_at` is written as Python's `str(datetime)`.
#[must_use]
pub fn snapshot_hash(snap: &TrackerSnapshot, links: &SyncLinks) -> String {
    let pull_requests: Vec<Value> = snap
        .pull_requests
        .iter()
        .map(|reference| {
            json!({
                "url": reference.url,
                "normalized_url": reference.normalized_url,
                "provider": reference.provider,
                "repo_full_name": reference.repo_full_name,
                "repo_normalized_url": reference.repo_normalized_url,
                "number": reference.number,
                "sources": reference.sources,
                "title": reference.title,
            })
        })
        .collect();
    let snapshot = json!({
        "external_id": snap.external_id,
        "key": snap.key,
        "url": snap.url,
        "project_key": snap.project_key,
        "title": snap.title,
        "description": snap.description,
        "priority": snap.priority.as_str(),
        "assignee": snap.assignee,
        "status_name": snap.status_name,
        "status_category": snap.status_category.as_str(),
        "resolution": snap.resolution,
        "parent_key": snap.parent_key,
        "blocked_by_keys": snap.blocked_by_keys,
        "pull_requests": pull_requests,
        "remote_updated_at": snap.remote_updated_at.format_isoformat().replacen('T', " ", 1),
        "issue_type": snap.issue_type,
        "labels": snap.labels,
    });
    let payload = json!({
        "snapshot": snapshot,
        "links": {
            "repositories": links.repositories,
            "parent": links.parent,
            "blockers": links.blockers,
        },
    });
    wg_hash::sha256_hex(canonical_json(&payload).as_bytes())
}

/// The task fields that a tracker sync compares with a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFields<'a> {
    pub id: &'a str,
    pub key: &'a str,
    pub title: &'a str,
    pub description: &'a str,
    pub priority: TaskPriority,
    pub assignee: Option<&'a str>,
    pub parent_id: Option<&'a str>,
}

/// The parent task a snapshot's parent key resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentFields<'a> {
    pub id: &'a str,
    pub key: &'a str,
}

/// What a sync changes on a tracked task. Every field is `None` or false when it already matches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FieldUpdates {
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<TaskPriority>,
    /// `Some(None)` clears the assignee.
    pub assignee: Option<Option<String>>,
    /// The snapshot has no parent and the task has one.
    pub clear_parent: bool,
    /// The key of the parent task to set.
    pub parent_key: Option<String>,
    /// Refusals: a parent that would make a hierarchy cycle.
    pub errors: Vec<String>,
}

impl FieldUpdates {
    /// Whether any field changes; refusals alone do not count.
    #[must_use]
    pub fn has_changes(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.priority.is_some()
            || self.assignee.is_some()
            || self.clear_parent
            || self.parent_key.is_some()
    }
}

/// The task fields that differ from the snapshot. Only Jira-owned fields are considered.
///
/// `parent` is the task the snapshot's parent key resolved to, or `None` when the parent is
/// unknown or absent. `is_ancestor` reports whether the task is an ancestor of the parent; it
/// is only asked when the parent differs and is not the task itself.
pub fn field_updates(
    task: &TaskFields,
    snap: &TrackerSnapshot,
    parent: Option<ParentFields>,
    is_ancestor: impl FnOnce() -> bool,
) -> FieldUpdates {
    let (clear_parent, parent_key, errors) = match parent {
        None => (
            snap.parent_key.is_none() && task.parent_id.is_some(),
            None,
            Vec::new(),
        ),
        Some(parent) if Some(parent.id) != task.parent_id => {
            if parent.id == task.id || is_ancestor() {
                (
                    false,
                    None,
                    vec![format!(
                        "{}: parent {} would create a hierarchy cycle",
                        task.key, parent.key
                    )],
                )
            } else {
                (false, Some(parent.key.to_string()), Vec::new())
            }
        }
        Some(_) => (false, None, Vec::new()),
    };
    FieldUpdates {
        title: (snap.title != task.title).then(|| snap.title.clone()),
        description: (snap.description != task.description).then(|| snap.description.clone()),
        priority: (snap.priority != task.priority).then_some(snap.priority),
        assignee: (snap.assignee.as_deref() != task.assignee).then(|| snap.assignee.clone()),
        clear_parent,
        parent_key,
        errors,
    }
}

/// A dependency row of a task, as the shell reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingDependency<'a> {
    pub depends_on_id: &'a str,
    /// Whether the sync created the row. Human-made dependencies are never touched.
    pub sync_created: bool,
}

/// A blocker of a task, resolved to a local task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockerTask<'a> {
    pub id: &'a str,
    pub key: &'a str,
}

/// The dependency rows to create and delete so that a task's sync-created dependencies match its
/// blockers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DependencyPlan {
    /// Blocker ids to add as dependencies, in blocker order.
    pub add: Vec<String>,
    /// Sync-created dependency ids to remove, in the order given.
    pub remove: Vec<String>,
    /// Refusals: a blocker that would make a dependency cycle.
    pub errors: Vec<String>,
}

/// Plans the sync-created dependencies of a task. `would_cycle(blocker_id)` reports whether
/// depending on that blocker would close a cycle back to the task.
pub fn dependency_plan(
    task_key: &str,
    existing: &[ExistingDependency],
    blockers: &[BlockerTask],
    would_cycle: impl Fn(&str) -> bool,
) -> DependencyPlan {
    // Each blocker not yet depended on, with whether depending on it would close a cycle.
    let verdicts: Vec<(&BlockerTask, bool)> = blockers
        .iter()
        .filter(|blocker| !existing.iter().any(|dep| dep.depends_on_id == blocker.id))
        .map(|blocker| (blocker, would_cycle(blocker.id)))
        .collect();
    let add = verdicts
        .iter()
        .filter(|(_, cycle)| !cycle)
        .map(|(blocker, _)| blocker.id.to_string())
        .collect();
    let errors = verdicts
        .iter()
        .filter(|(_, cycle)| *cycle)
        .map(|(blocker, _)| {
            format!(
                "{task_key}: blocked by {} would create a dependency cycle",
                blocker.key
            )
        })
        .collect();
    let remove = existing
        .iter()
        .filter(|dep| {
            dep.sync_created
                && !blockers
                    .iter()
                    .any(|blocker| blocker.id == dep.depends_on_id)
        })
        .map(|dep| dep.depends_on_id.to_string())
        .collect();
    DependencyPlan {
        add,
        remove,
        errors,
    }
}
