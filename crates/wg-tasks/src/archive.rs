//! Archiving and unarchiving a task. Mirrors `archive` in `core/services/tasks.py`: archiving
//! sets `archived_at` to the decision's clock, a no-op returns the task unchanged, and every
//! real change bumps the version.

use std::collections::BTreeSet;

use wg_model::TaskSlice;
use wg_model::entity::Task;
use wg_model::stamp::Stamp;
use wg_types::WgError;

use crate::ownership::{guard_tracker_owned, tracked_by};

/// Event type of an archive change, or none when the task already was in the requested state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveChange {
    Unchanged,
    Archived,
    Unarchived,
}

impl ArchiveChange {
    /// The event the shell records for this change.
    pub fn event_type(self) -> Option<&'static str> {
        match self {
            Self::Unchanged => None,
            Self::Archived => Some("task.archived"),
            Self::Unarchived => Some("task.unarchived"),
        }
    }
}

/// The task after an archive request, and what changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivePlan {
    pub task: Task,
    pub change: ArchiveChange,
}

/// Archives (`archived = true`) or unarchives a task. The tracker guard applies only when the
/// state actually changes.
pub fn archive_plan(
    stamp: &Stamp,
    tasks: &TaskSlice,
    task: &Task,
    archived: bool,
) -> Result<ArchivePlan, WgError> {
    if archived == task.archived_at.is_some() {
        return Ok(ArchivePlan {
            task: task.clone(),
            change: ArchiveChange::Unchanged,
        });
    }
    let fields = BTreeSet::from(["archived"]);
    guard_tracker_owned(&stamp.actor, task, &fields, tracked_by(tasks, task.id))?;

    let (archived_at, change) = if archived {
        (Some(stamp.at), ArchiveChange::Archived)
    } else {
        (None, ArchiveChange::Unarchived)
    };
    let next = Task {
        version: task.version + 1,
        archived_at,
        ..task.clone()
    };
    Ok(ArchivePlan { task: next, change })
}
