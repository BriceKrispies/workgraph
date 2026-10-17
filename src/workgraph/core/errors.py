"""Explicit exception hierarchy shared by every interface (CLI, HTTP, MCP).

Each error carries a stable machine-readable ``code`` so that agents can react to
failures without parsing human text.
"""

from __future__ import annotations

from typing import Any


class WorkgraphError(Exception):
    """Base class for all expected, user-facing Workgraph failures."""

    code: str = "workgraph_error"
    http_status: int = 400

    def __init__(self, message: str, *, details: dict[str, Any] | None = None) -> None:
        super().__init__(message)
        self.message = message
        self.details = details or {}

    def to_dict(self) -> dict[str, Any]:
        return {"code": self.code, "message": self.message, "details": self.details}


class NotFoundError(WorkgraphError):
    code = "not_found"
    http_status = 404


class ValidationFailed(WorkgraphError):
    code = "validation_failed"
    http_status = 422


class ConflictError(WorkgraphError):
    code = "conflict"
    http_status = 409


class ConcurrencyError(ConflictError):
    """Raised when an optimistic-concurrency version check fails."""

    code = "version_conflict"


class InvalidTransitionError(ConflictError):
    code = "invalid_transition"


class DependencyCycleError(ValidationFailed):
    code = "dependency_cycle"


class PermissionDenied(WorkgraphError):
    code = "permission_denied"
    http_status = 403


class ProviderUnavailableError(WorkgraphError):
    """An external provider (agent CLI, LLM endpoint) cannot be used right now."""

    code = "provider_unavailable"
    http_status = 503


class GitError(WorkgraphError):
    code = "git_error"
    http_status = 500


class ConfigError(WorkgraphError):
    code = "config_error"


class ResourceBusyError(ConflictError):
    code = "resource_busy"
