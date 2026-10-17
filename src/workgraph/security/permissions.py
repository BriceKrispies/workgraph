"""Tool- and service-level authorization.

Every application-service mutation calls :func:`require` with the acting
:class:`~workgraph.core.context.RequestContext`. Interfaces only decide *who* the
actor is; the services decide *what* that actor may do.
"""

from __future__ import annotations

from enum import StrEnum
from typing import TYPE_CHECKING

from workgraph.core.errors import PermissionDenied

if TYPE_CHECKING:
    from workgraph.core.context import RequestContext


class Permission(StrEnum):
    WORKSPACE_READ = "workspace:read"
    WORKSPACE_ADMIN = "workspace:admin"
    PROJECT_WRITE = "project:write"
    REPO_READ = "repo:read"
    REPO_REGISTER = "repo:register"
    REPO_ADMIN = "repo:admin"
    TASK_READ = "task:read"
    TASK_WRITE = "task:write"
    TASK_CLAIM = "task:claim"
    TASK_COMPLETE = "task:complete"
    TASK_ACCEPT = "task:accept"
    TASK_ADMIN = "task:admin"
    GIT_READ = "git:read"
    GIT_INGEST = "git:ingest"
    GIT_LINK = "git:link"
    ATTEMPT_WRITE = "attempt:write"
    STACK_WRITE = "stack:write"
    RUN_START = "run:start"
    RUN_CANCEL = "run:cancel"
    VERIFY_RUN = "verify:run"
    AUTOMATION_READ = "automation:read"
    AUTOMATION_TRIGGER = "automation:trigger"
    AUTOMATION_MANAGE = "automation:manage"
    CONFIG_MANAGE = "config:manage"
    ANALYTICS_READ = "analytics:read"
    CLASSIFY_REQUEST = "classification:request"
    CLASSIFY_CONFIRM = "classification:confirm"
    JOB_MANAGE = "job:manage"
    TELEMETRY_INGEST = "telemetry:ingest"


ALL_PERMISSIONS: frozenset[Permission] = frozenset(Permission)

READ_PERMISSIONS: frozenset[Permission] = frozenset(
    {
        Permission.WORKSPACE_READ,
        Permission.REPO_READ,
        Permission.TASK_READ,
        Permission.GIT_READ,
        Permission.AUTOMATION_READ,
        Permission.ANALYTICS_READ,
    }
)

# Agents can track and report work by default, but cannot start code execution,
# accept their own work, change trusted configuration or author automation policy.
DEFAULT_AGENT_PERMISSIONS: frozenset[Permission] = READ_PERMISSIONS | frozenset(
    {
        Permission.TASK_WRITE,
        Permission.TASK_CLAIM,
        Permission.TASK_COMPLETE,
        Permission.ATTEMPT_WRITE,
        Permission.GIT_LINK,
        Permission.CLASSIFY_REQUEST,
        Permission.AUTOMATION_TRIGGER,
        Permission.TELEMETRY_INGEST,
    }
)

HUMAN_PERMISSIONS: frozenset[Permission] = ALL_PERMISSIONS
SYSTEM_PERMISSIONS: frozenset[Permission] = ALL_PERMISSIONS


def parse_permissions(values: list[str]) -> frozenset[Permission]:
    parsed: set[Permission] = set()
    for value in values:
        try:
            parsed.add(Permission(value))
        except ValueError as exc:
            raise PermissionDenied(f"unknown permission {value!r}") from exc
    return frozenset(parsed)


def require(ctx: RequestContext, *permissions: Permission) -> None:
    missing = [p for p in permissions if p not in ctx.permissions]
    if missing:
        raise PermissionDenied(
            f"actor {ctx.actor!r} lacks permission(s): {', '.join(sorted(missing))}",
            details={"actor": ctx.actor, "missing": sorted(missing)},
        )
