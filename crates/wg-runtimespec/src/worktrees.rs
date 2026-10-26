//! The pure half of the worktree service (WG-3773): crash recovery of managed worktrees as
//! `WorktreeManager.recover` in `integrations/git/worktrees.py` decides it, the stack a worktree
//! path belongs to, and the outputs the worktree commands print.
//!
//! The shell (`wg-runtime`'s `worktrees` module) reads the rows, the attempts, tasks and stacks
//! they belong to, checks the checkouts on disk and lists the worktree directory. Everything here
//! takes those facts as values.

use std::collections::BTreeSet;
use std::path::Path;

use wg_exec::recovery::{RecoveryAction, WorktreeFacts, recovery_action};
use wg_json::Value;
use wg_model::decision::Decision;
use wg_model::entity::{ManagedWorktree, ResourceLease};
use wg_model::event::{EventDraft, EventPayload, WorktreeRecovered, WorktreeRemoved};
use wg_model::stamp::Stamp;
use wg_model::write::{RowKey, Write};
use wg_registry::boundary::is_within;
use wg_storecodec::run_sql::WorktreeWithRepository;
use wg_types::{AttemptStatus, Id, WorktreeStatus};
use wg_wire::convention::{
    TimeFormat, dict, id, optional_id, optional_text, optional_timestamp, text, texts, timestamp,
};

/// The `last_error` of a row whose checkout is missing (`recover`).
pub const MISSING_ON_DISK: &str = "worktree missing on disk during recovery";

/// What recovery reads about one managed worktree row that is preparing or active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryRow {
    pub row: ManagedWorktree,
    /// The checkout exists on disk and is a Git repository.
    pub exists_on_disk: bool,
    /// The owning attempt's status, when the row has an attempt that exists.
    pub attempt_status: Option<AttemptStatus>,
    /// The owning attempt's task was accepted or cancelled, or is archived.
    pub task_closed: bool,
    /// The stack the path belongs to landed or was abandoned.
    pub stack_ended: bool,
}

/// A directory under an attempt directory: the path as Python prints it, and its comparison key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: String,
    pub key: String,
}

/// Everything recovery decides from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryInput {
    /// The rows whose status is preparing or active.
    pub rows: Vec<RecoveryRow>,
    /// The key of every managed worktree path, whatever its status.
    pub known: BTreeSet<String>,
    /// The directories under the attempt directories, in scan order.
    pub candidates: Vec<Candidate>,
}

/// What recovery did, by path (the `fixed` dict of `WorktreeManager.recover`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    pub activated: Vec<String>,
    pub released: Vec<String>,
    pub orphaned: Vec<String>,
    pub unknown_dirs: Vec<String>,
}

/// The comparison key of a path: Python's path equality, which ignores case on Windows.
pub fn path_key(path: &str) -> String {
    if cfg!(windows) {
        path.to_ascii_lowercase()
    } else {
        path.to_owned()
    }
}

/// The stack id of a stack worktree (`<worktrees>/stacks/<stack id>/<directory>`), else `None`.
/// Both paths are resolved already (`_stack_of`).
pub fn stack_of(path: &str, stacks_root: &str) -> Option<String> {
    if !is_within(path, stacks_root) {
        return None;
    }
    let parent = Path::new(path).parent()?;
    let grandparent = parent.parent()?;
    if path_key(&grandparent.to_string_lossy()) != path_key(stacks_root) {
        return None;
    }
    parent.file_name()?.to_str().map(str::to_owned)
}

/// Decides recovery: each open row becomes orphaned, released, activated or left alone, and the
/// unrecorded directories are reported. A `worktree.recovered` event follows when any row changed.
pub fn recovery_decision(input: &RecoveryInput, stamp: &Stamp) -> Decision<RecoveryReport> {
    let mut report = RecoveryReport::default();
    let mut writes: Vec<Write> = Vec::new();
    for item in &input.rows {
        let facts = WorktreeFacts {
            status: item.row.status,
            exists_on_disk: item.exists_on_disk,
            attempt_status: item.attempt_status,
            task_closed: item.task_closed,
            stack_ended: item.stack_ended,
        };
        let path = item.row.path.clone();
        let change = |status: WorktreeStatus, last_error: Option<String>| {
            Write::update(ManagedWorktree {
                status,
                last_error,
                ..item.row.clone()
            })
        };
        match recovery_action(facts) {
            RecoveryAction::Orphan => {
                report.orphaned.push(path);
                writes.push(change(
                    WorktreeStatus::Orphaned,
                    Some(MISSING_ON_DISK.to_owned()),
                ));
            }
            RecoveryAction::Release => {
                report.released.push(path);
                writes.push(change(
                    WorktreeStatus::Released,
                    item.row.last_error.clone(),
                ));
            }
            RecoveryAction::Activate => {
                report.activated.push(path);
                writes.push(change(WorktreeStatus::Active, item.row.last_error.clone()));
            }
            RecoveryAction::Keep => {}
        }
    }
    report.unknown_dirs = input
        .candidates
        .iter()
        .filter(|candidate| !input.known.contains(&candidate.key))
        .map(|candidate| candidate.path.clone())
        .collect();
    let changed = !writes.is_empty();
    let counts = WorktreeRecovered {
        activated: len(&report.activated),
        orphaned: len(&report.orphaned),
        released: len(&report.released),
        unknown_dirs: len(&report.unknown_dirs),
    };
    let decision = writes
        .into_iter()
        .fold(Decision::new(()), |decision, write| decision.write(write));
    let decision = if changed {
        let (event_id, _) = stamp.ids().next();
        decision.event(EventDraft::new(
            stamp,
            event_id,
            None,
            Id::nil(),
            EventPayload::WorktreeRecovered(counts),
        ))
    } else {
        decision
    };
    decision.with_reply(report)
}

fn len(paths: &[String]) -> i64 {
    i64::try_from(paths.len()).unwrap_or(i64::MAX)
}

/// The report of `worker recover`'s `worktrees` entry: the four path lists, in Python's key order.
pub fn recovery_report_out(report: &RecoveryReport) -> Value {
    dict(vec![
        (
            "activated",
            texts(report.activated.iter().map(String::as_str)),
        ),
        (
            "released",
            texts(report.released.iter().map(String::as_str)),
        ),
        (
            "orphaned",
            texts(report.orphaned.iter().map(String::as_str)),
        ),
        (
            "unknown_dirs",
            texts(report.unknown_dirs.iter().map(String::as_str)),
        ),
    ])
}

/// `WorktreeManager.list`: one dict per managed worktree, in the order the rows came (newest
/// first), with the name of its repository. Datetimes print as Python's `isoformat()`.
pub fn worktree_list_out(rows: &[WorktreeWithRepository]) -> Value {
    Value::List(rows.iter().map(worktree_item).collect())
}

fn worktree_item(row: &WorktreeWithRepository) -> Value {
    let worktree = &row.worktree;
    dict(vec![
        ("id", id(worktree.id)),
        ("repository", text(&row.repository_name)),
        ("repository_id", id(worktree.repository_id)),
        ("attempt_id", optional_id(worktree.attempt_id)),
        ("path", text(&worktree.path)),
        ("branch", text(&worktree.branch)),
        ("base_sha", text(&worktree.base_sha)),
        ("status", text(worktree.status.as_str())),
        ("last_error", optional_text(worktree.last_error.as_deref())),
        (
            "created_at",
            timestamp(worktree.created_at, TimeFormat::Isoformat),
        ),
        (
            "removed_at",
            optional_timestamp(worktree.removed_at, TimeFormat::Isoformat),
        ),
    ])
}

/// A managed worktree `release` targets: its id, its path and the checkout it was made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseTarget {
    pub id: Id,
    pub path: String,
    pub source_checkout_id: Id,
}

/// Decides `release`: the leases the attempt holds are deleted, and each of its worktree rows that
/// is not released yet becomes released. The reply is every row the attempt has, as targets for
/// the removal that follows when asked for.
pub fn release_decision(
    leases: &[ResourceLease],
    rows: &[ManagedWorktree],
) -> Decision<Vec<ReleaseTarget>> {
    let released = leases.iter().fold(Decision::new(()), |decision, lease| {
        decision.write(Write::delete(RowKey::ResourceLease(
            lease.resource_key.clone(),
        )))
    });
    let released = rows
        .iter()
        .filter(|row| row.status != WorktreeStatus::Released)
        .fold(released, |decision, row| {
            decision.write(Write::update(ManagedWorktree {
                status: WorktreeStatus::Released,
                ..row.clone()
            }))
        });
    let targets = rows
        .iter()
        .map(|row| ReleaseTarget {
            id: row.id,
            path: row.path.clone(),
            source_checkout_id: row.source_checkout_id,
        })
        .collect();
    released.with_reply(targets)
}

/// Decides a removal that went through: the row is removed at the stamp's time and the
/// `worktree.removed` event names its branch, which is kept.
pub fn removed_decision(row: &ManagedWorktree, stamp: &Stamp) -> Decision<()> {
    let (event_id, _) = stamp.ids().next();
    Decision::new(())
        .write(Write::update(ManagedWorktree {
            status: WorktreeStatus::Removed,
            removed_at: Some(stamp.at),
            ..row.clone()
        }))
        .event(EventDraft {
            attempt_id: row.attempt_id,
            ..EventDraft::new(
                stamp,
                event_id,
                Some(row.workspace_id),
                row.id,
                EventPayload::WorktreeRemoved(WorktreeRemoved {
                    path: row.path.clone(),
                    branch_kept: row.branch.clone(),
                }),
            )
        })
}

/// What `WorktreeManager.remove` answered: whether the worktree was removed, and why not when it
/// was not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub worktree_id: Id,
    pub removed: bool,
    pub reason: Option<String>,
}

/// The dict `remove` returns: `worktree_id`, `removed` and `reason` when it was not removed.
pub fn removal_out(removal: &Removal) -> Value {
    let mut fields = vec![
        ("worktree_id", id(removal.worktree_id)),
        ("removed", Value::Bool(removal.removed)),
    ];
    if let Some(reason) = &removal.reason {
        fields.push(("reason", text(reason)));
    }
    dict(fields)
}

impl Removal {
    /// A worktree that was removed.
    pub fn removed(worktree_id: Id) -> Self {
        Removal {
            worktree_id,
            removed: true,
            reason: None,
        }
    }

    /// A worktree that was kept, and why.
    pub fn refused(worktree_id: Id, reason: impl Into<String>) -> Self {
        Removal {
            worktree_id,
            removed: false,
            reason: Some(reason.into()),
        }
    }
}
