"""Workgraph MCP server (official Python SDK, ``MCPServer``).

Every tool calls the same application services as the CLI and HTTP API, under an
agent identity with explicit permissions. There is deliberately no tool for raw
SQL or shell execution; analytics are exposed through structured, allowlisted
parameters, and agent runs require the operator's execution opt-in.
"""

from __future__ import annotations

import json
import logging
from collections.abc import Callable
from functools import wraps
from typing import Annotated, Any, Literal, ParamSpec, TypeVar

from mcp.server.mcpserver import MCPServer
from mcp.server.mcpserver.exceptions import ResourceError, ResourceNotFoundError, ToolError
from pydantic import Field

from workgraph import __version__
from workgraph.analytics.service import AnalyticsFilters, AnalyticsQuery
from workgraph.app import Workgraph
from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError, WorkgraphError
from workgraph.core.models.enums import AccessMode, RelationType, TaskPriority, TaskStatus
from workgraph.core.models.schemas import (
    AttemptCreate,
    ClaimRequest,
    CommentCreate,
    ProgressReport,
    RelationCreate,
    RepositoryRegister,
    ResultReport,
    TaskCreate,
    TaskFilter,
    TaskUpdate,
)
from workgraph.security.permissions import DEFAULT_AGENT_PERMISSIONS, Permission

P = ParamSpec("P")
R = TypeVar("R")

INSTRUCTIONS = """Workgraph tracks engineering work across repositories.
Typical agent loop: task_get_ready -> task_claim -> attempt_create -> (work, committing with a
`Task-ID: <KEY>` trailer) -> task_report_progress -> task_link_commit -> task_report_result.
Reporting success moves a task to awaiting verification/review; only humans or configured
verification accept work. Task references accept a UUID or a key such as WG-12."""

Workspace = Annotated[str | None, Field(description="Workspace slug or id; omit for the default workspace")]
TaskRef = Annotated[str, Field(description="Task UUID or key, e.g. WG-12")]
Limit = Annotated[int, Field(ge=1, le=200, description="Maximum items to return")]
Offset = Annotated[int, Field(ge=0, description="Pagination offset (use next_offset from the previous page)")]


def _dump(value: Any) -> Any:
    if hasattr(value, "model_dump"):
        return value.model_dump(mode="json")
    if isinstance(value, list):
        return [_dump(v) for v in value]
    if isinstance(value, dict):
        return {k: _dump(v) for k, v in value.items()}
    return value


def tool_errors[**P, R](fn: Callable[P, R]) -> Callable[P, R]:
    @wraps(fn)
    def wrapper(*args: P.args, **kwargs: P.kwargs) -> R:
        try:
            return fn(*args, **kwargs)
        except WorkgraphError as exc:
            raise ToolError(json.dumps({"error": exc.to_dict()}, default=str)) from exc

    return wrapper


def build_server(
    app: Workgraph,
    *,
    agent_name: str = "mcp-client",
    permissions: frozenset[Permission] | None = None,
) -> MCPServer:
    perms = permissions if permissions is not None else DEFAULT_AGENT_PERMISSIONS
    server: MCPServer = MCPServer(
        name="workgraph",
        title="Workgraph",
        version=__version__,
        instructions=INSTRUCTIONS,
        log_level="WARNING",
    )
    for noisy in ("httpx", "mcp", "uvicorn.access"):
        logging.getLogger(noisy).setLevel(logging.WARNING)

    def ctx() -> RequestContext:
        return RequestContext.agent(agent_name, perms)

    def tool(name: str, description: str) -> Callable[[Callable[P, R]], Callable[P, R]]:
        def decorator(fn: Callable[P, R]) -> Callable[P, R]:
            server.add_tool(tool_errors(fn), name=name, description=description, structured_output=True)
            return fn

        return decorator

    # ------------------------------------------------------------------ workspaces / projects

    @tool("workspace_list", "List workspaces.")
    def workspace_list() -> dict[str, Any]:
        return {"workspaces": _dump(app.workspaces.list(ctx()))}

    @tool("workspace_get", "Get a workspace with task counts, queue status and running attempts.")
    def workspace_get(workspace: Workspace = None) -> dict[str, Any]:
        return _dump(app.workspaces.summary(ctx(), workspace or app.default_workspace_id()))

    @tool("project_list", "List projects in a workspace.")
    def project_list(workspace: Workspace = None) -> dict[str, Any]:
        return {"projects": _dump(app.projects.list(ctx(), workspace or app.default_workspace_id()))}

    @tool("project_get", "Get a project summary: repositories, task counts and recent tasks.")
    def project_get(
        project: Annotated[str, Field(description="Project slug or id")], workspace: Workspace = None
    ) -> dict[str, Any]:
        return _dump(app.projects.summary(ctx(), workspace or app.default_workspace_id(), project))

    # ------------------------------------------------------------------ repositories

    @tool("repository_list", "List registered repositories (optionally only those in a project).")
    def repository_list(workspace: Workspace = None, project: str | None = None) -> dict[str, Any]:
        return {"repositories": _dump(app.repositories.list(ctx(), workspace, project=project))}

    @tool("repository_get", "Get a repository registration: checkouts, remotes, health, branches.")
    def repository_get(
        repository: Annotated[str, Field(description="Repository name or id")], workspace: Workspace = None
    ) -> dict[str, Any]:
        return _dump(app.repositories.get(ctx(), workspace, repository))

    @tool("repository_register", "Register a local Git repository path (requires repo:register permission).")
    def repository_register(
        path: Annotated[str, Field(description="Absolute path to a Git working tree")],
        name: str | None = None,
        read_only: bool = False,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return _dump(
            app.repositories.register(
                ctx(),
                workspace,
                RepositoryRegister(
                    path=path,
                    name=name,
                    access_mode=AccessMode.READ_ONLY if read_only else AccessMode.READ_WRITE,
                ),
            )
        )

    @tool(
        "repository_status",
        "Working-tree status of a repository's primary checkout (branch, staged, unstaged).",
    )
    def repository_status(repository: str, workspace: Workspace = None) -> dict[str, Any]:
        return _dump(app.repositories.status(ctx(), workspace, repository))

    # ------------------------------------------------------------------ tasks

    @tool("task_create", "Create a task. Repositories/projects/dependencies may be given by name or key.")
    def task_create(
        title: Annotated[str, Field(min_length=1, max_length=500)],
        description: str = "",
        acceptance_criteria: list[str] | None = None,
        priority: TaskPriority = TaskPriority.MEDIUM,
        ready: bool = False,
        tags: list[str] | None = None,
        projects: list[str] | None = None,
        repositories: list[str] | None = None,
        parent: str | None = None,
        depends_on: list[str] | None = None,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return _dump(
            app.tasks.create(
                ctx(),
                workspace,
                TaskCreate(
                    title=title,
                    description=description,
                    acceptance_criteria=acceptance_criteria or [],
                    priority=priority,
                    status="ready" if ready else "backlog",
                    tags=tags or [],
                    projects=projects or [],
                    repositories=repositories or [],
                    parent=parent,
                    depends_on=depends_on or [],
                ),
            )
        )

    @tool("task_get", "Get full task details: description, criteria, comments, dependencies, rollup.")
    def task_get(task: TaskRef, workspace: Workspace = None) -> dict[str, Any]:
        return _dump(app.tasks.detail(ctx(), workspace, task))

    @tool("task_search", "Search tasks by text (full-text, plus semantic when embeddings exist).")
    def task_search(
        query: Annotated[str, Field(min_length=1)],
        mode: Literal["hybrid", "lexical", "semantic"] = "hybrid",
        limit: Limit = 20,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return _dump(app.search.search(ctx(), workspace, query, mode=mode, limit=limit))

    @tool("task_list", "List tasks with filters, sorting and pagination.")
    def task_list(
        statuses: list[TaskStatus] | None = None,
        priorities: list[TaskPriority] | None = None,
        project: str | None = None,
        repository: str | None = None,
        tags: list[str] | None = None,
        category: str | None = None,
        parent: str | None = None,
        text: str | None = None,
        sort: Literal[
            "created_at", "updated_at", "priority", "key", "title", "status", "due_at"
        ] = "created_at",
        descending: bool = True,
        limit: Limit = 50,
        offset: Offset = 0,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return _dump(
            app.tasks.list(
                ctx(),
                workspace,
                TaskFilter(
                    statuses=statuses,
                    priorities=priorities,
                    project=project,
                    repository=repository,
                    tags=tags,
                    category=category,
                    parent=parent,
                    text=text,
                    sort=sort,
                    descending=descending,
                    limit=limit,
                    offset=offset,
                ),
            )
        )

    @tool("task_update", "Update task fields or status. Pass expected_version for optimistic concurrency.")
    def task_update(
        task: TaskRef,
        expected_version: int | None = None,
        title: str | None = None,
        description: str | None = None,
        status: Literal["backlog", "ready", "blocked", "cancelled"] | None = None,
        priority: TaskPriority | None = None,
        add_tags: list[str] | None = None,
        remove_tags: list[str] | None = None,
        acceptance_criteria: list[str] | None = None,
        reason: str | None = None,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        fields: dict[str, Any] = {
            "expected_version": expected_version,
            "title": title,
            "description": description,
            "status": status,
            "priority": priority,
            "add_tags": add_tags,
            "remove_tags": remove_tags,
            "acceptance_criteria": acceptance_criteria,
            "reason": reason,
        }
        return _dump(
            app.tasks.update(
                ctx(), workspace, task, TaskUpdate(**{k: v for k, v in fields.items() if v is not None})
            )
        )

    @tool("task_claim", "Claim a ready task for this agent (exclusive, with a lease).")
    def task_claim(
        task: TaskRef, lease_minutes: Annotated[int, Field(ge=1, le=10080)] = 120, workspace: Workspace = None
    ) -> dict[str, Any]:
        return _dump(app.tasks.claim(ctx(), workspace, task, ClaimRequest(lease_minutes=lease_minutes)))

    @tool("task_release", "Release a claimed task back to ready.")
    def task_release(task: TaskRef, reason: str | None = None, workspace: Workspace = None) -> dict[str, Any]:
        return _dump(app.tasks.release(ctx(), workspace, task, reason))

    @tool(
        "task_complete",
        "Report completion. The task moves to awaiting verification/review, never to accepted.",
    )
    def task_complete(
        task: TaskRef, summary: str = "", attempt_id: str | None = None, workspace: Workspace = None
    ) -> dict[str, Any]:
        return _dump(app.tasks.complete(ctx(), workspace, task, summary=summary, attempt_id=attempt_id))

    @tool(
        "task_get_ready",
        "Ready, unclaimed tasks whose dependencies are all accepted, highest priority first.",
    )
    def task_get_ready(
        project: str | None = None,
        repository: str | None = None,
        limit: Limit = 20,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return {
            "tasks": _dump(
                app.tasks.get_ready(
                    ctx(), workspace, project=project, repository=repository, agent=agent_name, limit=limit
                )
            )
        }

    @tool("task_add_dependency", "Make TASK depend on DEPENDS_ON (cycles are rejected).")
    def task_add_dependency(
        task: TaskRef, depends_on: TaskRef, workspace: Workspace = None
    ) -> dict[str, Any]:
        return _dump(app.tasks.add_dependency(ctx(), workspace, task, depends_on))

    @tool("task_remove_dependency", "Remove a dependency.")
    def task_remove_dependency(
        task: TaskRef, depends_on: TaskRef, workspace: Workspace = None
    ) -> dict[str, Any]:
        return _dump(app.tasks.remove_dependency(ctx(), workspace, task, depends_on))

    @tool(
        "task_add_relation",
        "Relate two tasks (relates_to, duplicates, follows_up, regression_of, similar_to).",
    )
    def task_add_relation(
        task: TaskRef,
        target: TaskRef,
        relation_type: RelationType = RelationType.RELATES_TO,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return _dump(
            app.tasks.add_relation(
                ctx(), workspace, task, RelationCreate(target=target, relation_type=relation_type)
            )
        )

    @tool("task_add_comment", "Add a comment to a task.")
    def task_add_comment(
        task: TaskRef, body: Annotated[str, Field(min_length=1)], workspace: Workspace = None
    ) -> dict[str, Any]:
        return _dump(app.tasks.add_comment(ctx(), workspace, task, CommentCreate(body=body)))

    @tool("task_get_history", "Immutable event history of a task and its attempts.")
    def task_get_history(
        task: TaskRef, limit: Annotated[int, Field(ge=1, le=2000)] = 500, workspace: Workspace = None
    ) -> dict[str, Any]:
        return {"events": _dump(app.history.task_history(ctx(), workspace, task, limit=limit))}

    @tool("task_get_attempts", "Attempts made on a task (including failed and nested attempts).")
    def task_get_attempts(
        task: TaskRef, limit: Limit = 50, offset: Offset = 0, workspace: Workspace = None
    ) -> dict[str, Any]:
        return _dump(app.attempts.list(ctx(), workspace, task=task, limit=limit, offset=offset))

    @tool("task_link_repository", "Associate a repository with a task.")
    def task_link_repository(
        task: TaskRef, repository: str, read_only: bool = False, workspace: Workspace = None
    ) -> dict[str, Any]:
        return _dump(
            app.tasks.link_repository(ctx(), workspace, task, repository, "read_only" if read_only else None)
        )

    @tool("task_link_commit", "Explicitly link a commit (SHA or ref) to a task with confidence 1.0.")
    def task_link_commit(
        task: TaskRef,
        sha: Annotated[str, Field(min_length=4, max_length=64)],
        repository: str | None = None,
        note: str = "",
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return {
            "links": _dump(app.git.link_commit(ctx(), workspace, task, sha, repository=repository, note=note))
        }

    @tool("task_report_progress", "Report progress (message and optional percent) on a task or attempt.")
    def task_report_progress(
        task: TaskRef,
        message: Annotated[str, Field(min_length=1)],
        percent: Annotated[float | None, Field(ge=0, le=100)] = None,
        attempt_id: str | None = None,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return _dump(
            app.tasks.report_progress(
                ctx(),
                workspace,
                task,
                ProgressReport(message=message, percent=percent, attempt_id=attempt_id),
            )
        )

    @tool("task_report_result", "Report an attempt outcome. Include usage only if your runtime measured it.")
    def task_report_result(
        task: TaskRef,
        outcome: Literal["succeeded", "failed"],
        summary: str = "",
        attempt_id: str | None = None,
        input_tokens: int | None = None,
        output_tokens: int | None = None,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        usage = {
            k: v
            for k, v in {"input_tokens": input_tokens, "output_tokens": output_tokens}.items()
            if v is not None
        }
        return _dump(
            app.tasks.report_result(
                ctx(),
                workspace,
                task,
                ResultReport(outcome=outcome, summary=summary, attempt_id=attempt_id, usage=usage or None),
            )
        )

    # ------------------------------------------------------------------ attempts & runs

    @tool("attempt_create", "Register an attempt for work you are doing on a claimed task.")
    def attempt_create(
        task: TaskRef,
        model: str | None = None,
        session_id: str | None = None,
        parent_attempt: str | None = None,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return _dump(
            app.attempts.create(
                ctx(),
                workspace,
                AttemptCreate(
                    task=task,
                    provider=agent_name,
                    model=model,
                    session_id=session_id,
                    parent_attempt=parent_attempt,
                ),
            )
        )

    @tool("attempt_get", "Get an attempt with telemetry, manifests and provenance.")
    def attempt_get(attempt_id: str) -> dict[str, Any]:
        return _dump(app.attempts.get(ctx(), attempt_id))

    @tool("attempt_list", "List attempts in a workspace (filter by task, status or provider).")
    def attempt_list(
        task: str | None = None,
        statuses: list[str] | None = None,
        provider: str | None = None,
        limit: Limit = 50,
        offset: Offset = 0,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return _dump(
            app.attempts.list(
                ctx(), workspace, task=task, statuses=statuses, provider=provider, limit=limit, offset=offset
            )
        )

    @tool("run_start", "Start a Workgraph-managed agent run (requires run:start and execution enabled).")
    def run_start(
        task: TaskRef, agent: str | None = None, model: str | None = None, workspace: Workspace = None
    ) -> dict[str, Any]:
        return _dump(app.runs.start(ctx(), workspace, task, agent=agent, model=model))

    @tool("run_get", "Get a run: attempt state, job status and recent progress events.")
    def run_get(attempt_id: str, events: Annotated[int, Field(ge=1, le=500)] = 50) -> dict[str, Any]:
        return _dump(app.runs.get(ctx(), attempt_id, events=events))

    @tool("run_cancel", "Cancel a queued or running agent run.")
    def run_cancel(attempt_id: str, reason: str = "cancelled via MCP") -> dict[str, Any]:
        return _dump(app.runs.cancel(ctx(), attempt_id, reason))

    # ------------------------------------------------------------------ git

    @tool(
        "git_get_changes",
        "Commits linked to a task with provenance, cumulative churn and final change surface.",
    )
    def git_get_changes(
        task: TaskRef, min_confidence: Annotated[float, Field(ge=0, le=1)] = 0.5, workspace: Workspace = None
    ) -> dict[str, Any]:
        return app.git.task_changes(ctx(), workspace, task, min_confidence=min_confidence)

    @tool("git_get_task_diff", "Full diff for a task, read live from Git (truncated to max_bytes).")
    def git_get_task_diff(
        task: TaskRef,
        repository: str | None = None,
        max_bytes: Annotated[int, Field(ge=1000, le=2_000_000)] = 200_000,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return app.git.task_diff(ctx(), workspace, task, repository=repository, max_bytes=max_bytes)

    # ------------------------------------------------------------------ classification

    @tool(
        "classification_get",
        "Current classification and full history for a task (model results are inferences).",
    )
    def classification_get(task: TaskRef, workspace: Workspace = None) -> dict[str, Any]:
        return _dump(app.classification.get(ctx(), workspace, task))

    @tool("classification_request", "Queue a background classification job for a task.")
    def classification_request(task: TaskRef, workspace: Workspace = None) -> dict[str, Any]:
        return app.classification.request(ctx(), workspace, task)

    # ------------------------------------------------------------------ analytics

    @tool(
        "analytics_query",
        "Run a predefined metric with structured grouping/filters (see analytics_report for the catalog).",
    )
    def analytics_query(
        metric: Annotated[
            str, Field(description="Metric id, e.g. tasks_accepted, execution_duration, cumulative_churn")
        ],
        group_by: list[
            Literal[
                "period",
                "project",
                "repository",
                "category",
                "priority",
                "status",
                "provider",
                "model",
                "agent",
                "kind",
                "check_name",
                "module",
                "file_class",
                "relation_type",
            ]
        ]
        | None = None,
        period: Literal["day", "week", "month", "quarter", "year"] = "week",
        project: str | None = None,
        repository: str | None = None,
        category: str | None = None,
        provider: str | None = None,
        model: str | None = None,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        return app.analytics.query(
            ctx(),
            workspace,
            AnalyticsQuery(
                metric=metric,
                group_by=list(group_by or []),
                period=period,
                filters=AnalyticsFilters(
                    project=project, repository=repository, category=category, provider=provider, model=model
                ),
            ),
        )

    @tool(
        "analytics_report",
        "Named analytical reports: catalog, overview, coupling, failures, estimate or history.",
    )
    def analytics_report(
        report: Literal["catalog", "overview", "coupling", "failures", "estimate", "history"],
        task: str | None = None,
        workspace: Workspace = None,
    ) -> dict[str, Any]:
        c = ctx()
        if report == "catalog":
            return {"metrics": app.analytics.definitions()}
        if report == "overview":
            return app.analytics.overview(c, workspace)
        if report == "coupling":
            return app.analytics.coupling(c, workspace)
        if report == "failures":
            return app.analytics.failures(c, workspace)
        if report == "estimate":
            if not task:
                raise ToolError("estimate requires a task")
            return app.analytics.estimate(c, workspace, task=task)
        return {"tasks": app.analytics.task_history(c, workspace)}

    # ------------------------------------------------------------------ automation

    @tool("automation_list", "List automation rules and their recent runs.")
    def automation_list(workspace: Workspace = None) -> dict[str, Any]:
        c = ctx()
        return {
            "rules": _dump(app.automation.list_rules(c, workspace)),
            "recent_runs": _dump(app.automation.runs(c, workspace, limit=20)),
        }

    @tool("automation_trigger", "Trigger a manual automation rule (rules are authored by humans).")
    def automation_trigger(rule: str, task: str | None = None, workspace: Workspace = None) -> dict[str, Any]:
        return _dump(app.automation.trigger(ctx(), workspace, rule, inputs={"task": task} if task else None))

    # ------------------------------------------------------------------ resources

    def resource_errors(fn: Callable[P, R]) -> Callable[P, R]:
        @wraps(fn)
        def wrapper(*args: P.args, **kwargs: P.kwargs) -> R:
            try:
                return fn(*args, **kwargs)
            except NotFoundError as exc:
                raise ResourceNotFoundError(exc.message) from exc
            except WorkgraphError as exc:
                raise ResourceError(exc.message) from exc

        return wrapper

    @server.resource("workgraph://workspaces", mime_type="application/json", description="All workspaces")
    @resource_errors
    def res_workspaces() -> str:
        return json.dumps(_dump(app.workspaces.list(ctx())))

    @server.resource(
        "workgraph://workspaces/{workspace}/summary",
        mime_type="application/json",
        description="Workspace summary",
    )
    @resource_errors
    def res_workspace(workspace: str) -> str:
        return json.dumps(_dump(app.workspaces.summary(ctx(), workspace)))

    @server.resource(
        "workgraph://workspaces/{workspace}/projects/{project}",
        mime_type="application/json",
        description="Project summary",
    )
    @resource_errors
    def res_project(workspace: str, project: str) -> str:
        return json.dumps(_dump(app.projects.summary(ctx(), workspace, project)))

    @server.resource(
        "workgraph://workspaces/{workspace}/tasks/{task}",
        mime_type="application/json",
        description="Task details",
    )
    @resource_errors
    def res_task(workspace: str, task: str) -> str:
        return json.dumps(_dump(app.tasks.detail(ctx(), workspace, task)))

    @server.resource(
        "workgraph://workspaces/{workspace}/tasks/{task}/history",
        mime_type="application/json",
        description="Task execution history (events and attempts)",
    )
    @resource_errors
    def res_history(workspace: str, task: str) -> str:
        c = ctx()
        return json.dumps(
            {
                "events": _dump(app.history.task_history(c, workspace, task)),
                "attempts": _dump(app.attempts.list(c, workspace, task=task)),
            }
        )

    @server.resource(
        "workgraph://workspaces/{workspace}/analytics/{metric}",
        mime_type="application/json",
        description="A supported analytics metric grouped by week",
    )
    @resource_errors
    def res_metric(workspace: str, metric: str) -> str:
        return json.dumps(
            app.analytics.query(ctx(), workspace, AnalyticsQuery(metric=metric, group_by=["period"])),
            default=str,
        )

    @server.resource(
        "workgraph://analytics/catalog",
        mime_type="application/json",
        description="Definitions of every supported metric",
    )
    def res_catalog() -> str:
        return json.dumps(app.analytics.definitions())

    return server
