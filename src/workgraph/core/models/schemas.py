"""Pydantic schemas: validated service inputs and stable result shapes.

These are the contract shared by the CLI (``--json``), the HTTP API and MCP tools.
"""

from __future__ import annotations

import re
from datetime import datetime
from typing import Any, Literal, TypeVar

from pydantic import BaseModel, ConfigDict, Field, field_validator

from workgraph.core.models.enums import (
    AccessMode,
    AttemptStatus,
    CommentKind,
    RelationType,
    StackItemState,
    StackStatus,
    TaskPriority,
    TaskStatus,
)

T = TypeVar("T")

SLUG_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,62}$")
TAG_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,99}$")


class Schema(BaseModel):
    model_config = ConfigDict(from_attributes=True, populate_by_name=True)


class InputSchema(BaseModel):
    model_config = ConfigDict(extra="forbid", str_strip_whitespace=True)


class Page[T](Schema):
    items: list[T]
    total: int
    limit: int
    offset: int
    next_offset: int | None = None


# --------------------------------------------------------------------------- workspaces


class WorkspaceCreate(InputSchema):
    slug: str
    name: str = Field(min_length=1, max_length=200)
    description: str = ""
    key_prefix: str = Field(default="WG", min_length=1, max_length=10)

    @field_validator("slug")
    @classmethod
    def _slug(cls, value: str) -> str:
        if not SLUG_RE.match(value):
            raise ValueError("slug must be lowercase letters, digits and dashes")
        return value

    @field_validator("key_prefix")
    @classmethod
    def _prefix(cls, value: str) -> str:
        value = value.upper()
        if not re.match(r"^[A-Z][A-Z0-9]{0,9}$", value):
            raise ValueError("key prefix must be alphanumeric and start with a letter")
        return value


class WorkspaceOut(Schema):
    id: str
    slug: str
    name: str
    description: str
    key_prefix: str
    settings: dict[str, Any]
    created_at: datetime
    updated_at: datetime
    archived_at: datetime | None = None


class WorkspaceSummary(Schema):
    workspace: WorkspaceOut
    projects: int
    repositories: int
    tasks_by_status: dict[str, int]
    ready_tasks: int
    running_attempts: int
    queued_jobs: int
    dead_jobs: int


# --------------------------------------------------------------------------- projects


class ProjectCreate(InputSchema):
    slug: str
    name: str = Field(min_length=1, max_length=200)
    description: str = ""
    repositories: list[str] = Field(default_factory=list)
    settings: dict[str, Any] = Field(default_factory=dict)

    @field_validator("slug")
    @classmethod
    def _slug(cls, value: str) -> str:
        if not SLUG_RE.match(value):
            raise ValueError("slug must be lowercase letters, digits and dashes")
        return value


class ProjectUpdate(InputSchema):
    name: str | None = Field(default=None, min_length=1, max_length=200)
    description: str | None = None
    status: Literal["active", "archived"] | None = None
    settings: dict[str, Any] | None = None


class ProjectOut(Schema):
    id: str
    workspace_id: str
    slug: str
    name: str
    description: str
    status: str
    settings: dict[str, Any]
    repository_ids: list[str] = Field(default_factory=list)
    created_at: datetime
    updated_at: datetime


class ProjectSummary(Schema):
    project: ProjectOut
    repositories: list[RepositoryBrief]
    tasks_by_status: dict[str, int]
    open_tasks: int
    accepted_tasks: int
    recent_tasks: list[TaskBrief]


# --------------------------------------------------------------------------- repositories


class RepositoryRegister(InputSchema):
    path: str = Field(min_length=1)
    name: str | None = Field(default=None, max_length=200)
    description: str = ""
    access_mode: AccessMode = AccessMode.READ_WRITE
    tracked_branches: list[str] = Field(default_factory=list)
    projects: list[str] = Field(default_factory=list)


class RemoteOut(Schema):
    name: str
    url: str
    normalized_url: str


class CheckoutOut(Schema):
    id: str
    path: str
    kind: str
    is_primary: bool
    available: bool
    access_mode: str | None = None
    last_seen_at: datetime | None = None


class RepositoryBrief(Schema):
    id: str
    name: str
    access_mode: str
    health_status: str


class RepositoryOut(Schema):
    id: str
    workspace_id: str
    name: str
    description: str
    identity_fingerprint: str | None
    default_branch: str | None
    tracked_branches: list[str]
    access_mode: str
    health_status: str
    health_detail: dict[str, Any]
    health_checked_at: datetime | None
    is_shallow: bool
    last_scanned_at: datetime | None
    archived_at: datetime | None
    remotes: list[RemoteOut]
    checkouts: list[CheckoutOut]
    project_ids: list[str] = Field(default_factory=list)
    depends_on: list[str] = Field(default_factory=list)
    created_at: datetime
    updated_at: datetime


class RepositoryUpdate(InputSchema):
    description: str | None = None
    access_mode: AccessMode | None = None
    default_branch: str | None = None
    tracked_branches: list[str] | None = None


class RepositoryStatus(Schema):
    repository_id: str
    name: str
    checkout_path: str | None
    available: bool
    branch: str | None = None
    head: str | None = None
    detached: bool = False
    staged: list[str] = Field(default_factory=list)
    unstaged: list[str] = Field(default_factory=list)
    untracked: list[str] = Field(default_factory=list)
    ahead: int | None = None
    behind: int | None = None
    is_shallow: bool = False
    error: str | None = None


# --------------------------------------------------------------------------- tasks


class ExternalRef(InputSchema):
    system: str = Field(min_length=1, max_length=100)
    id: str = Field(min_length=1, max_length=200)
    url: str | None = None


class EffortEstimate(InputSchema):
    value: float = Field(ge=0)
    unit: Literal["points", "hours", "size"] = "points"
    source: Literal["human", "historical", "model"] = "human"


def _validate_tags(tags: list[str]) -> list[str]:
    cleaned: list[str] = []
    for tag in tags:
        tag = tag.strip()
        if not TAG_RE.match(tag):
            raise ValueError(f"invalid tag {tag!r}")
        if tag not in cleaned:
            cleaned.append(tag)
    return cleaned


class TaskCreate(InputSchema):
    title: str = Field(min_length=1, max_length=500)
    description: str = Field(default="", max_length=200_000)
    acceptance_criteria: list[str] = Field(default_factory=list)
    status: Literal["backlog", "ready"] = "backlog"
    priority: TaskPriority = TaskPriority.MEDIUM
    tags: list[str] = Field(default_factory=list)
    projects: list[str] = Field(default_factory=list, description="Project ids or slugs")
    repositories: list[str] = Field(default_factory=list, description="Repository ids or names")
    parent: str | None = Field(default=None, description="Parent task id or key")
    depends_on: list[str] = Field(default_factory=list, description="Task ids or keys this task depends on")
    assignee: str | None = None
    assigned_agent: str | None = None
    auto_execute: bool = False
    due_at: datetime | None = None
    scheduled_at: datetime | None = None
    estimated_effort: EffortEstimate | None = None
    external_refs: list[ExternalRef] = Field(default_factory=list)
    metadata: dict[str, Any] = Field(default_factory=dict)

    @field_validator("tags")
    @classmethod
    def _tags(cls, value: list[str]) -> list[str]:
        return _validate_tags(value)


class TaskUpdate(InputSchema):
    expected_version: int | None = Field(
        default=None, description="Optimistic concurrency: the task version the caller last read"
    )
    title: str | None = Field(default=None, min_length=1, max_length=500)
    description: str | None = Field(default=None, max_length=200_000)
    acceptance_criteria: list[str] | None = None
    status: TaskStatus | None = None
    priority: TaskPriority | None = None
    tags: list[str] | None = None
    add_tags: list[str] | None = None
    remove_tags: list[str] | None = None
    projects: list[str] | None = None
    parent: str | None = None
    clear_parent: bool = False
    assignee: str | None = None
    assigned_agent: str | None = None
    auto_execute: bool | None = None
    due_at: datetime | None = None
    scheduled_at: datetime | None = None
    estimated_effort: EffortEstimate | None = None
    external_refs: list[ExternalRef] | None = None
    metadata: dict[str, Any] | None = None
    reason: str | None = Field(default=None, max_length=2000)

    @field_validator("tags", "add_tags", "remove_tags")
    @classmethod
    def _tags(cls, value: list[str] | None) -> list[str] | None:
        return None if value is None else _validate_tags(value)


class TaskBrief(Schema):
    id: str
    key: str
    title: str
    status: str
    priority: str
    category: str | None = None


class TaskOut(Schema):
    id: str
    workspace_id: str
    key: str
    number: int
    title: str
    description: str
    acceptance_criteria: list[str]
    status: str
    priority: str
    category: str | None
    tags: list[str]
    project_ids: list[str]
    repository_ids: list[str]
    parent_id: str | None
    child_ids: list[str] = Field(default_factory=list)
    depends_on: list[str] = Field(default_factory=list)
    blocks: list[str] = Field(default_factory=list)
    assignee: str | None
    assigned_agent: str | None
    claimed_by: str | None
    claim_expires_at: datetime | None
    auto_execute: bool
    estimated_effort: dict[str, Any] | None
    external_refs: list[dict[str, Any]]
    metadata: dict[str, Any]
    due_at: datetime | None
    scheduled_at: datetime | None
    created_by: str
    version: int
    created_at: datetime
    updated_at: datetime
    started_at: datetime | None
    completed_at: datetime | None
    accepted_at: datetime | None
    archived_at: datetime | None
    current_classification_id: str | None


class TaskDetail(TaskOut):
    relations: list[RelationOut] = Field(default_factory=list)
    comments: list[CommentOut] = Field(default_factory=list)
    rollup: TaskRollup | None = None
    dependency_state: DependencyState | None = None


class TaskFilter(InputSchema):
    statuses: list[TaskStatus] | None = None
    priorities: list[TaskPriority] | None = None
    project: str | None = None
    repository: str | None = None
    tags: list[str] | None = None
    category: str | None = None
    assignee: str | None = None
    parent: str | None = None
    root_only: bool = False
    text: str | None = None
    created_after: datetime | None = None
    created_before: datetime | None = None
    include_archived: bool = False
    archived_only: bool = False
    sort: Literal["created_at", "updated_at", "priority", "key", "title", "status", "due_at"] = "created_at"
    descending: bool = True
    limit: int = Field(default=50, ge=1, le=500)
    offset: int = Field(default=0, ge=0)


class RelationOut(Schema):
    id: str
    source_task_id: str
    target_task_id: str
    relation_type: str
    created_at: datetime


class RelationCreate(InputSchema):
    target: str
    relation_type: RelationType = RelationType.RELATES_TO


class CommentCreate(InputSchema):
    body: str = Field(min_length=1, max_length=100_000)
    kind: CommentKind = CommentKind.COMMENT
    data: dict[str, Any] = Field(default_factory=dict)
    attempt_id: str | None = None


class CommentOut(Schema):
    id: str
    task_id: str
    attempt_id: str | None
    author: str
    kind: str
    body: str
    data: dict[str, Any]
    created_at: datetime


class TaskRollup(Schema):
    total_descendants: int
    by_status: dict[str, int]
    accepted: int
    progress: float = Field(description="Fraction of descendant tasks accepted (0..1)")


class DependencyState(Schema):
    satisfied: bool
    unsatisfied: list[TaskBrief]


class BulkUpdate(InputSchema):
    tasks: list[str] = Field(min_length=1, max_length=500)
    update: TaskUpdate


class BulkResult(Schema):
    updated: list[str]
    failed: dict[str, str]


class ClaimRequest(InputSchema):
    agent: str | None = None
    lease_minutes: int = Field(default=120, ge=1, le=7 * 24 * 60)
    expected_version: int | None = None


class GraphNode(Schema):
    id: str
    key: str
    title: str
    status: str
    priority: str
    category: str | None
    parent_id: str | None
    project_ids: list[str]
    repository_ids: list[str]
    blocked: bool


class GraphEdge(Schema):
    source: str
    target: str
    kind: Literal["depends_on", "parent", "relation"]
    relation_type: str | None = None


class TaskGraph(Schema):
    nodes: list[GraphNode]
    edges: list[GraphEdge]


# --------------------------------------------------------------------------- events


class EventOut(Schema):
    seq: int
    id: str
    workspace_id: str | None
    event_type: str
    entity_type: str
    entity_id: str
    actor: str
    actor_kind: str
    occurred_at: datetime
    correlation_id: str
    causation_id: str | None
    attempt_id: str | None
    schema_version: int
    data: dict[str, Any]


# --------------------------------------------------------------------------- attempts


class AttemptCreate(InputSchema):
    task: str
    provider: str = Field(default="external", max_length=100)
    adapter: str | None = None
    agent_id: str | None = None
    model: str | None = None
    session_id: str | None = None
    parent_attempt: str | None = None
    status: Literal["queued", "running"] = "running"


class AttemptOut(Schema):
    id: str
    workspace_id: str
    task_id: str
    number: int
    status: AttemptStatus
    provider: str
    adapter: str
    agent_id: str | None
    model: str | None
    session_id: str | None
    parent_attempt_id: str | None
    retry_of_attempt_id: str | None
    rerun_of_attempt_id: str | None
    created_by: str
    created_at: datetime
    queued_at: datetime | None
    started_at: datetime | None
    finished_at: datetime | None
    queue_wait_ms: int | None
    duration_ms: int | None
    exit_reason: str | None
    exit_code: int | None
    error: str | None
    summary: str | None
    input_task_version: int | None
    baseline_manifest_id: str | None
    outcome_manifest_id: str | None
    usage: dict[str, Any]
    usage_source: str | None
    cost_usd: float | None
    cost_source: str | None
    telemetry: dict[str, Any]
    modified_repository_ids: list[str]
    config_snapshot: dict[str, Any]
    environment: dict[str, Any]


class ProgressReport(InputSchema):
    message: str = Field(min_length=1, max_length=20_000)
    percent: float | None = Field(default=None, ge=0, le=100)
    attempt_id: str | None = None
    data: dict[str, Any] = Field(default_factory=dict)


class ResultReport(InputSchema):
    attempt_id: str | None = None
    outcome: Literal["succeeded", "failed"]
    summary: str = Field(default="", max_length=50_000)
    artifacts: list[dict[str, Any]] = Field(default_factory=list)
    usage: dict[str, Any] | None = Field(
        default=None, description="Provider-measured token usage. Omit when not measured."
    )
    cost_usd: float | None = Field(default=None, ge=0, description="Only when reliably measured")


# --------------------------------------------------------------------------- stacks


class StackCreate(InputSchema):
    parent: str = Field(min_length=1, description="Parent task id or key")


class StackItemAdd(InputSchema):
    task: str = Field(min_length=1, description="Child task id or key")


class StackRepositoryOut(Schema):
    repository_id: str
    repository_name: str
    base_sha: str
    integration_branch: str
    integration_tip: str | None = Field(
        default=None, description="Head of the integration branch, when its worktree has been prepared"
    )


class StackItemOut(Schema):
    id: str
    stack_id: str
    task_id: str
    task_key: str
    task_title: str
    state: StackItemState
    merge_order: int | None = None
    detail: str | None = None
    created_at: datetime
    updated_at: datetime


class StackOut(Schema):
    id: str
    workspace_id: str
    parent_task_id: str
    parent_key: str
    parent_title: str
    status: StackStatus
    repositories: list[StackRepositoryOut] = Field(default_factory=list)
    items: list[StackItemOut] = Field(default_factory=list)
    created_at: datetime
    updated_at: datetime


ProjectSummary.model_rebuild()
TaskDetail.model_rebuild()
