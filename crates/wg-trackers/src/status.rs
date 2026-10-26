//! How a tracker's status constrains a tracked task's Workgraph status.

use wg_types::TaskStatus;

/// The status a tracked task must move to, or `None` to leave it where it is.
///
/// `done` closes the task (accepted, or cancelled for a cancelling resolution). Leaving `done`
/// reopens a closed task to backlog, and `new` returns a closed task to backlog. `indeterminate`
/// never moves a task, so agents can claim and run it. The category is compared as received:
/// only the exact text `done` closes a task.
#[must_use]
pub fn tracker_target_status(
    current: TaskStatus,
    category: &str,
    resolution: Option<&str>,
    cancelled_resolutions: &[&str],
) -> Option<TaskStatus> {
    if category == "done" {
        let cancelled = resolution.is_some_and(|resolution| {
            let resolution = resolution.to_lowercase();
            cancelled_resolutions
                .iter()
                .any(|known| known.to_lowercase() == resolution)
        });
        let target = if cancelled {
            TaskStatus::Cancelled
        } else {
            TaskStatus::Accepted
        };
        return (current != target).then_some(target);
    }
    match current {
        TaskStatus::Accepted | TaskStatus::Cancelled => Some(TaskStatus::Backlog),
        _ => None,
    }
}
