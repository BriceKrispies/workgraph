"""Declarative automation rule schemas and the deterministic condition evaluator."""

from __future__ import annotations

import re
from typing import Annotated, Any, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from workgraph.automation.cron import Cron
from workgraph.core.models.enums import TaskPriority

# Event types that rules may subscribe to (names of domain events).
TRIGGER_EVENTS = {
    "task.created": "Task created",
    "task.updated": "Task updated",
    "task.became_ready": "Task becomes ready",
    "task.completed": "Task completed",
    "attempt.failed": "Attempt failed",
    "verification.completed": "Verification completed",
    "git.commit_ingested": "Git commit ingested",
    "classification.completed": "Classification completed",
    "repository.changes_detected": "Repository changes detected",
    "task.accepted": "Task accepted",
    "task.status_changed": "Task status changed",
}


class _Strict(BaseModel):
    model_config = ConfigDict(extra="forbid")


class Trigger(_Strict):
    type: Literal["event", "schedule", "manual"]
    event_types: list[str] = Field(default_factory=list)
    cron: str | None = None
    interval_seconds: int | None = Field(default=None, ge=60)

    @model_validator(mode="after")
    def _check(self) -> Trigger:
        if self.type == "event":
            if not self.event_types:
                raise ValueError("event triggers need at least one event type")
            unknown = [e for e in self.event_types if e not in TRIGGER_EVENTS]
            if unknown:
                raise ValueError(f"unknown trigger events {unknown}; allowed: {sorted(TRIGGER_EVENTS)}")
        if self.type == "schedule":
            if bool(self.cron) == bool(self.interval_seconds):
                raise ValueError("schedule triggers need exactly one of cron or interval_seconds")
            if self.cron:
                Cron(self.cron)
        return self


Operator = Literal[
    "eq",
    "ne",
    "in",
    "not_in",
    "contains",
    "not_contains",
    "gt",
    "gte",
    "lt",
    "lte",
    "exists",
    "not_exists",
    "matches",
    "starts_with",
]


class Predicate(_Strict):
    field: str = Field(pattern=r"^[a-z_][a-z0-9_]*(\.[a-z0-9_]+)*$")
    op: Operator
    value: Any = None

    @field_validator("value")
    @classmethod
    def _value(cls, value: Any) -> Any:
        if isinstance(value, str) and len(value) > 500:
            raise ValueError("condition values are limited to 500 characters")
        return value

    @model_validator(mode="after")
    def _regex(self) -> Predicate:
        if self.op == "matches":
            if not isinstance(self.value, str):
                raise ValueError("matches requires a string pattern")
            re.compile(self.value)
        return self


class Condition(_Strict):
    """``{"all": [...]}``, ``{"any": [...]}``, ``{"not": {...}}`` or a single predicate."""

    all: list[Condition] | None = None
    any: list[Condition] | None = None
    not_: Condition | None = Field(default=None, alias="not")
    predicate: Predicate | None = None

    model_config = ConfigDict(extra="forbid", populate_by_name=True)

    @model_validator(mode="before")
    @classmethod
    def _shorthand(cls, data: Any) -> Any:
        if isinstance(data, dict) and "field" in data and "op" in data:
            return {"predicate": data}
        return data


_MISSING = object()


def resolve_path(context: dict[str, Any], path: str) -> Any:
    current: Any = context
    for part in path.split("."):
        if isinstance(current, dict) and part in current:
            current = current[part]
        else:
            return _MISSING
    return current


def _compare(actual: Any, op: str, expected: Any) -> bool:
    if op == "exists":
        return actual is not _MISSING and actual is not None
    if op == "not_exists":
        return actual is _MISSING or actual is None
    if actual is _MISSING:
        return False
    try:
        match op:
            case "eq":
                return actual == expected
            case "ne":
                return actual != expected
            case "in":
                return actual in (expected or [])
            case "not_in":
                return actual not in (expected or [])
            case "contains":
                return expected in actual if isinstance(actual, list | str | dict) else False
            case "not_contains":
                return expected not in actual if isinstance(actual, list | str | dict) else True
            case "gt":
                return actual > expected
            case "gte":
                return actual >= expected
            case "lt":
                return actual < expected
            case "lte":
                return actual <= expected
            case "starts_with":
                return isinstance(actual, str) and actual.startswith(str(expected))
            case "matches":
                return isinstance(actual, str) and re.search(str(expected), actual[:10_000]) is not None
    except TypeError:
        return False
    return False


def evaluate(
    condition: Condition | None, context: dict[str, Any], trace: list[dict[str, Any]] | None = None
) -> bool:
    """Evaluate deterministically, appending each predicate's outcome to ``trace``."""
    if condition is None:
        return True
    trace = trace if trace is not None else []
    if condition.predicate is not None:
        p = condition.predicate
        actual = resolve_path(context, p.field)
        result = _compare(actual, p.op, p.value)
        trace.append(
            {
                "field": p.field,
                "op": p.op,
                "expected": p.value,
                "actual": None if actual is _MISSING else actual,
                "result": result,
            }
        )
        return result
    if condition.all is not None:
        return all(evaluate(c, context, trace) for c in condition.all)
    if condition.any is not None:
        return any(evaluate(c, context, trace) for c in condition.any)
    if condition.not_ is not None:
        return not evaluate(condition.not_, context, trace)
    return True


# --------------------------------------------------------------------------- actions


class ClassifyAction(_Strict):
    type: Literal["classify_task"]


class AddTagsAction(_Strict):
    type: Literal["add_tags"]
    tags: list[str] = Field(min_length=1, max_length=20)


class UpdateTaskAction(_Strict):
    type: Literal["update_task"]
    priority: TaskPriority | None = None
    status: Literal["backlog", "ready", "blocked", "cancelled"] | None = None
    assignee: str | None = None
    assigned_agent: str | None = None
    auto_execute: bool | None = None


class ChildSpec(_Strict):
    title: str = Field(min_length=1, max_length=300)
    description: str = Field(default="", max_length=5000)
    priority: TaskPriority | None = None
    tags: list[str] = Field(default_factory=list)


class CreateChildrenAction(_Strict):
    type: Literal["create_child_tasks"]
    children: list[ChildSpec] = Field(min_length=1, max_length=20)
    sequential: bool = False
    inherit_repositories: bool = True


class CreateDependencyAction(_Strict):
    type: Literal["create_dependency"]
    depends_on: str | None = None
    blocks: str | None = None

    @model_validator(mode="after")
    def _one(self) -> CreateDependencyAction:
        if bool(self.depends_on) == bool(self.blocks):
            raise ValueError("specify exactly one of depends_on or blocks")
        return self


class EnqueueRunAction(_Strict):
    type: Literal["enqueue_agent_run"]
    agent: str | None = None
    model: str | None = None


class VerifyAction(_Strict):
    type: Literal["run_verification"]


class ReportAction(_Strict):
    type: Literal["generate_report"]
    period_days: int = Field(default=7, ge=1, le=366)


class NotifyAction(_Strict):
    type: Literal["notify"]
    level: Literal["info", "warning", "error"] = "info"
    title: str = Field(min_length=1, max_length=300)
    body: str = Field(default="", max_length=5000)


Action = Annotated[
    ClassifyAction
    | AddTagsAction
    | UpdateTaskAction
    | CreateChildrenAction
    | CreateDependencyAction
    | EnqueueRunAction
    | VerifyAction
    | ReportAction
    | NotifyAction,
    Field(discriminator="type"),
]
PRIVILEGED_ACTIONS = {"enqueue_agent_run"}


class RuleSpec(_Strict):
    name: str = Field(min_length=1, max_length=200)
    description: str = ""
    enabled: bool = True
    trigger: Trigger
    conditions: Condition | None = None
    actions: list[Action] = Field(min_length=1, max_length=20)
    retry_max_attempts: int = Field(default=3, ge=1, le=10)
    max_depth: int = Field(default=3, ge=1, le=10)


_TEMPLATE = re.compile(r"\{\{\s*([a-z_][a-z0-9_.]*)\s*\}\}")


def render(template: str, context: dict[str, Any], limit: int = 500) -> str:
    """Substitute ``{{path}}`` placeholders with context values (data only, never code)."""

    def sub(match: re.Match[str]) -> str:
        value = resolve_path(context, match.group(1))
        return "" if value is _MISSING or value is None else str(value)[:limit]

    return _TEMPLATE.sub(sub, template)
