"""How a tracker's status constrains a tracked task's Workgraph status."""

from __future__ import annotations

from collections.abc import Collection

from workgraph.core.models.enums import TaskStatus

S = TaskStatus


def tracker_target_status(
    current: TaskStatus,
    category: str,
    resolution: str | None,
    cancelled_resolutions: Collection[str],
) -> TaskStatus | None:
    """The status a tracked task must move to, or None to leave it where it is.

    ``done`` closes the task (accepted, or cancelled for a cancelling resolution); leaving ``done``
    reopens a closed task to backlog; ``new`` returns a closed task to backlog. ``indeterminate``
    never moves a task, so agents can claim and run it.
    """
    if category == "done":
        cancelled = resolution is not None and resolution.casefold() in {
            r.casefold() for r in cancelled_resolutions
        }
        target = S.CANCELLED if cancelled else S.ACCEPTED
        return None if current == target else target
    if current in (S.ACCEPTED, S.CANCELLED):
        return S.BACKLOG
    return None
