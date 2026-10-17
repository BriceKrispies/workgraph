"""Task and attempt lifecycle state machines.

Task states describe the *work item*; attempt states describe one *execution* of
it. They are deliberately separate: an attempt can succeed while its task still
awaits verification, review, or acceptance.
"""

from __future__ import annotations

from workgraph.core.errors import InvalidTransitionError
from workgraph.core.models.enums import AttemptStatus, TaskStatus

S = TaskStatus

TASK_TRANSITIONS: dict[TaskStatus, frozenset[TaskStatus]] = {
    S.BACKLOG: frozenset({S.READY, S.BLOCKED, S.CANCELLED}),
    S.READY: frozenset({S.BACKLOG, S.BLOCKED, S.CLAIMED, S.RUNNING, S.CANCELLED}),
    S.BLOCKED: frozenset({S.BACKLOG, S.READY, S.CANCELLED}),
    S.CLAIMED: frozenset(
        {S.READY, S.BLOCKED, S.RUNNING, S.AWAITING_VERIFICATION, S.AWAITING_REVIEW, S.FAILED, S.CANCELLED}
    ),
    S.RUNNING: frozenset(
        {S.READY, S.BLOCKED, S.CLAIMED, S.AWAITING_VERIFICATION, S.AWAITING_REVIEW, S.FAILED, S.CANCELLED}
    ),
    S.AWAITING_VERIFICATION: frozenset(
        {S.AWAITING_REVIEW, S.ACCEPTED, S.FAILED, S.RUNNING, S.READY, S.CLAIMED, S.CANCELLED}
    ),
    S.AWAITING_REVIEW: frozenset({S.ACCEPTED, S.FAILED, S.RUNNING, S.READY, S.CLAIMED, S.CANCELLED}),
    S.FAILED: frozenset({S.READY, S.BACKLOG, S.CLAIMED, S.RUNNING, S.BLOCKED, S.CANCELLED}),
    # Reopening accepted/cancelled work is allowed and recorded as ``task.reopened``.
    S.ACCEPTED: frozenset({S.READY, S.BACKLOG}),
    S.CANCELLED: frozenset({S.BACKLOG, S.READY}),
}

# Entering ACCEPTED is reserved for the acceptance service, which enforces the
# configured verification / manual-acceptance conditions.
RESTRICTED_TARGETS = frozenset({S.ACCEPTED})

# Statuses that require every dependency to be satisfied.
REQUIRES_DEPENDENCIES = frozenset({S.READY, S.CLAIMED, S.RUNNING})


def check_task_transition(current: TaskStatus, target: TaskStatus, *, privileged: bool = False) -> None:
    if current == target:
        return
    if target in RESTRICTED_TARGETS and not privileged:
        raise InvalidTransitionError(
            f"tasks enter {target} only through acceptance (verification or recorded manual acceptance)",
            details={"from": current, "to": target},
        )
    if target not in TASK_TRANSITIONS[current]:
        raise InvalidTransitionError(
            f"cannot move task from {current} to {target}",
            details={"from": current, "to": target, "allowed": sorted(TASK_TRANSITIONS[current])},
        )


A = AttemptStatus

ATTEMPT_TRANSITIONS: dict[AttemptStatus, frozenset[AttemptStatus]] = {
    A.QUEUED: frozenset({A.PREPARING, A.RUNNING, A.CANCELLED, A.ABANDONED, A.FAILED}),
    A.PREPARING: frozenset({A.RUNNING, A.FAILED, A.CANCELLED, A.ABANDONED}),
    A.RUNNING: frozenset({A.VERIFYING, A.SUCCEEDED, A.FAILED, A.CANCELLED, A.TIMED_OUT, A.ABANDONED}),
    A.VERIFYING: frozenset({A.SUCCEEDED, A.FAILED, A.CANCELLED, A.ABANDONED}),
    A.SUCCEEDED: frozenset(),
    A.FAILED: frozenset(),
    A.CANCELLED: frozenset(),
    A.TIMED_OUT: frozenset(),
    A.ABANDONED: frozenset(),
}


def check_attempt_transition(current: AttemptStatus, target: AttemptStatus) -> None:
    if current == target:
        return
    if target not in ATTEMPT_TRANSITIONS[current]:
        raise InvalidTransitionError(
            f"cannot move attempt from {current} to {target}",
            details={"from": current, "to": target, "allowed": sorted(ATTEMPT_TRANSITIONS[current])},
        )
