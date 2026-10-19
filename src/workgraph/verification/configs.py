"""Verification configuration schema and scope resolution.

Verification commands are *trusted configuration*: they can only be created by an
actor holding ``config:manage`` (a human operator by default) and are never parsed
out of task descriptions or agent output.
"""

from __future__ import annotations

import re
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator
from sqlalchemy import or_, select
from sqlalchemy.orm import Session

from workgraph.core.errors import ValidationFailed
from workgraph.core.models.enums import CheckKind
from workgraph.persistence.orm import Task, TaskProject, TaskRepository, VerificationConfig

_ENV_NAME = re.compile(r"^[A-Z_][A-Z0-9_]{0,63}$")
_FORBIDDEN_EXECUTABLES = {"sh", "bash", "zsh", "fish", "cmd", "cmd.exe", "powershell", "pwsh", "nu"}

CheckTier = Literal["fast", "full"]


class CheckSpec(BaseModel):
    model_config = ConfigDict(extra="forbid")

    name: str = Field(min_length=1, max_length=200)
    kind: CheckKind = CheckKind.TEST
    command: list[str] = Field(min_length=1, description="Argument vector; no shell is involved")
    repository: str | None = Field(
        default=None, description="Repository id/name used as working directory; omit for integration checks"
    )
    timeout_seconds: float = Field(default=900, gt=0, le=24 * 3600)
    required: bool = True
    env: dict[str, str] = Field(default_factory=dict)
    parser: Literal["auto", "pytest", "generic", "cargo"] = "auto"
    allow_shell: bool = Field(
        default=False, description="Permit a shell interpreter as the executable (explicit operator opt-in)"
    )
    tier: CheckTier = Field(
        default="full",
        description="fast checks (lint, format, type check) gate stack children and are listed for the agent; "
        "full checks run once per verification",
    )

    @field_validator("command")
    @classmethod
    def _command(cls, value: list[str]) -> list[str]:
        if any("\x00" in part for part in value):
            raise ValueError("command arguments must not contain NUL bytes")
        if not value[0].strip():
            raise ValueError("command executable must not be empty")
        return value

    @field_validator("env")
    @classmethod
    def _env(cls, value: dict[str, str]) -> dict[str, str]:
        for key in value:
            if not _ENV_NAME.match(key):
                raise ValueError(f"invalid environment variable name {key!r}")
        return value

    def model_post_init(self, _context: object) -> None:
        executable = self.command[0].replace("\\", "/").rsplit("/", 1)[-1].lower()
        if executable in _FORBIDDEN_EXECUTABLES and not self.allow_shell:
            raise ValueError(
                f"check {self.name!r} uses shell interpreter {executable!r}; set allow_shell=true to opt in"
            )


@dataclass(frozen=True)
class ApplicableCheck:
    config_id: str
    config_name: str
    scope_type: str
    scope_id: str
    spec: CheckSpec


def parse_tier(value: str | None) -> CheckTier | None:
    """Validate a tier supplied by a CLI, API or job boundary; ``None`` selects every tier."""
    if value is None:
        return None
    if value == "fast":
        return "fast"
    if value == "full":
        return "full"
    raise ValidationFailed(f"unknown check tier {value!r}; expected one of: fast, full")


def checks_for_tier(checks: Sequence[ApplicableCheck], tier: CheckTier | None) -> list[ApplicableCheck]:
    """Keep the checks of one tier, preserving order; ``None`` keeps every check."""
    if tier is None:
        return list(checks)
    return [check for check in checks if check.spec.tier == tier]


def applicable_configs(session: Session, task: Task) -> list[VerificationConfig]:
    project_ids = list(session.scalars(select(TaskProject.project_id).where(TaskProject.task_id == task.id)))
    repo_ids = list(
        session.scalars(select(TaskRepository.repository_id).where(TaskRepository.task_id == task.id))
    )
    clauses = [
        (VerificationConfig.scope_type == "workspace") & (VerificationConfig.scope_id == task.workspace_id),
        (VerificationConfig.scope_type == "task") & (VerificationConfig.scope_id == task.id),
    ]
    if project_ids:
        clauses.append(
            (VerificationConfig.scope_type == "project") & VerificationConfig.scope_id.in_(project_ids)
        )
    if repo_ids:
        clauses.append(
            (VerificationConfig.scope_type == "repository") & VerificationConfig.scope_id.in_(repo_ids)
        )
    return list(
        session.scalars(
            select(VerificationConfig)
            .where(
                VerificationConfig.workspace_id == task.workspace_id,
                VerificationConfig.enabled.is_(True),
                or_(*clauses),
            )
            .order_by(VerificationConfig.scope_type, VerificationConfig.name)
        )
    )


def applicable_checks(session: Session, task: Task) -> list[ApplicableCheck]:
    checks: list[ApplicableCheck] = []
    for config in applicable_configs(session, task):
        for raw in config.checks:
            spec = CheckSpec.model_validate(raw)
            if config.scope_type == "repository" and spec.repository is None:
                spec = spec.model_copy(update={"repository": config.scope_id})
            checks.append(
                ApplicableCheck(
                    config_id=config.id,
                    config_name=config.name,
                    scope_type=config.scope_type,
                    scope_id=config.scope_id,
                    spec=spec,
                )
            )
    return checks
