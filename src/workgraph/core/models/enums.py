"""Enumerations shared by the domain, persistence and interface layers."""

from __future__ import annotations

from enum import StrEnum


class TaskStatus(StrEnum):
    BACKLOG = "backlog"
    READY = "ready"
    BLOCKED = "blocked"
    CLAIMED = "claimed"
    RUNNING = "running"
    AWAITING_VERIFICATION = "awaiting_verification"
    AWAITING_REVIEW = "awaiting_review"
    ACCEPTED = "accepted"
    FAILED = "failed"
    CANCELLED = "cancelled"


TERMINAL_TASK_STATUSES = frozenset({TaskStatus.ACCEPTED, TaskStatus.CANCELLED})
# A dependency is satisfied only once the blocking task's result has been accepted.
DEPENDENCY_SATISFIED_STATUSES = frozenset({TaskStatus.ACCEPTED})
OPEN_TASK_STATUSES = frozenset(set(TaskStatus) - TERMINAL_TASK_STATUSES)


class TaskPriority(StrEnum):
    CRITICAL = "critical"
    HIGH = "high"
    MEDIUM = "medium"
    LOW = "low"


PRIORITY_RANK = {TaskPriority.CRITICAL: 0, TaskPriority.HIGH: 1, TaskPriority.MEDIUM: 2, TaskPriority.LOW: 3}


class AttemptStatus(StrEnum):
    QUEUED = "queued"
    PREPARING = "preparing"
    RUNNING = "running"
    VERIFYING = "verifying"
    SUCCEEDED = "succeeded"
    FAILED = "failed"
    CANCELLED = "cancelled"
    TIMED_OUT = "timed_out"
    ABANDONED = "abandoned"


TERMINAL_ATTEMPT_STATUSES = frozenset(
    {
        AttemptStatus.SUCCEEDED,
        AttemptStatus.FAILED,
        AttemptStatus.CANCELLED,
        AttemptStatus.TIMED_OUT,
        AttemptStatus.ABANDONED,
    }
)


class RelationType(StrEnum):
    RELATES_TO = "relates_to"
    DUPLICATES = "duplicates"
    FOLLOWS_UP = "follows_up"
    REGRESSION_OF = "regression_of"
    SIMILAR_TO = "similar_to"


class AccessMode(StrEnum):
    READ_ONLY = "read_only"
    READ_WRITE = "read_write"


class CommentKind(StrEnum):
    COMMENT = "comment"
    PROGRESS = "progress"
    RESULT = "result"


class AttributionSource(StrEnum):
    EXPLICIT = "explicit"
    EXECUTION_CONTEXT = "execution_context"
    WORKTREE = "worktree"
    TRAILER = "trailer"
    MESSAGE_INFERENCE = "message_inference"
    REWRITE_RECONCILIATION = "rewrite_reconciliation"


ATTRIBUTION_CONFIDENCE = {
    AttributionSource.EXPLICIT: 1.0,
    AttributionSource.EXECUTION_CONTEXT: 0.95,
    AttributionSource.TRAILER: 0.95,
    AttributionSource.WORKTREE: 0.9,
    AttributionSource.REWRITE_RECONCILIATION: 0.85,
    AttributionSource.MESSAGE_INFERENCE: 0.5,
}


class JobStatus(StrEnum):
    QUEUED = "queued"
    RUNNING = "running"
    SUCCEEDED = "succeeded"
    FAILED = "failed"  # failed this attempt, will be retried
    DEAD = "dead"  # exhausted retries (dead letter)
    CANCELLED = "cancelled"


class WorktreeStatus(StrEnum):
    PREPARING = "preparing"
    ACTIVE = "active"
    RELEASED = "released"
    REMOVED = "removed"
    ORPHANED = "orphaned"


class ChangeSetStatus(StrEnum):
    OPEN = "open"
    PARTIALLY_APPLIED = "partially_applied"
    APPLIED = "applied"
    PARTIALLY_VERIFIED = "partially_verified"
    VERIFIED = "verified"
    FAILED = "failed"
    ABANDONED = "abandoned"


class ChangeSetItemStatus(StrEnum):
    PENDING = "pending"
    COMMITTED = "committed"
    VERIFIED = "verified"
    FAILED = "failed"


class StackStatus(StrEnum):
    OPEN = "open"
    INTEGRATING = "integrating"
    VERIFYING = "verifying"
    VERIFIED = "verified"
    LANDED = "landed"
    ABANDONED = "abandoned"


class StackItemState(StrEnum):
    PENDING = "pending"
    READY = "ready"
    MERGED = "merged"
    CONFLICT = "conflict"
    EJECTED = "ejected"
    VERIFIED = "verified"


class VerificationStatus(StrEnum):
    PENDING = "pending"
    RUNNING = "running"
    PASSED = "passed"
    FAILED = "failed"
    ERROR = "error"
    SKIPPED = "skipped"


class CheckKind(StrEnum):
    TEST = "test"
    LINT = "lint"
    TYPECHECK = "typecheck"
    BUILD = "build"
    INTEGRATION = "integration"
    CUSTOM = "custom"


class ClassificationStatus(StrEnum):
    PENDING = "pending"
    SUCCEEDED = "succeeded"
    FAILED = "failed"


class ActorKind(StrEnum):
    HUMAN = "human"
    AGENT = "agent"
    SYSTEM = "system"
    AUTOMATION = "automation"
