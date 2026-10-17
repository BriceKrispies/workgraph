"""Dependency-aware agent scheduler and task decomposition.

A task is eligible for automatic execution only when:

* it opted in (``auto_execute``) and agent execution is enabled,
* its status is ``ready`` (or ``failed`` with retry budget remaining),
* every dependency is accepted,
* every linked repository is registered, not archived and has an available checkout,
* no other attempt for it is active, and
* concurrency capacity remains (``execution.max_concurrent_runs``).

Selection is by priority, then age, interleaved round-robin across projects so a
single busy project cannot starve the others. Resource conflicts between running
attempts are prevented by job resource keys and checkout leases.
"""

from __future__ import annotations

from collections import OrderedDict, defaultdict
from graphlib import CycleError, TopologicalSorter
from typing import TYPE_CHECKING, Any

from pydantic import BaseModel, ConfigDict, Field, ValidationError
from sqlalchemy import case, func, select

from workgraph.core.context import RequestContext
from workgraph.core.errors import (
    DependencyCycleError,
    NotFoundError,
    ValidationFailed,
    WorkgraphError,
)
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import PRIORITY_RANK, AttemptStatus, TaskStatus
from workgraph.core.models.schemas import TaskCreate
from workgraph.core.services.base import Service
from workgraph.core.services.repositories import primary_checkout
from workgraph.core.services.tasks import unsatisfied_dependencies
from workgraph.persistence.orm import AgentAttempt, Repository, Task, TaskProject, TaskRepository

if TYPE_CHECKING:
    pass

ACTIVE = [AttemptStatus.QUEUED, AttemptStatus.PREPARING, AttemptStatus.RUNNING, AttemptStatus.VERIFYING]


class ChildProposal(BaseModel):
    model_config = ConfigDict(extra="forbid")

    ref: str = Field(pattern=r"^[A-Za-z0-9_-]{1,20}$")
    title: str = Field(min_length=1, max_length=300)
    description: str = Field(default="", max_length=5000)
    depends_on: list[str] = Field(default_factory=list)
    repositories: list[str] = Field(default_factory=list)


class TaskDecomposition(BaseModel):
    model_config = ConfigDict(extra="forbid", title="TaskDecomposition")

    children: list[ChildProposal] = Field(min_length=1, max_length=15)
    rationale: str = ""


DECOMPOSE_SYSTEM = (
    "You split a software task into smaller child tasks. Respond with JSON matching the schema. "
    "Use short refs (c1, c2, ...). depends_on lists refs of children that must be finished first. "
    "repositories must be chosen from the provided list. Do not invent work beyond the task."
)


def validate_decomposition(proposal: TaskDecomposition, allowed_repos: set[str]) -> list[str]:
    """Validate references and acyclicity; return children refs in dependency order."""
    refs = [c.ref for c in proposal.children]
    if len(refs) != len(set(refs)):
        raise ValidationFailed("decomposition contains duplicate child refs")
    graph: dict[str, set[str]] = {}
    for child in proposal.children:
        unknown = [d for d in child.depends_on if d not in refs]
        if unknown:
            raise ValidationFailed(f"child {child.ref} depends on unknown refs {unknown}")
        if child.ref in child.depends_on:
            raise DependencyCycleError(f"child {child.ref} depends on itself")
        bad_repos = [r for r in child.repositories if r not in allowed_repos]
        if bad_repos:
            raise ValidationFailed(
                f"child {child.ref} references repositories outside the parent task: {bad_repos}"
            )
        graph[child.ref] = set(child.depends_on)
    try:
        return list(TopologicalSorter(graph).static_order())
    except CycleError as exc:
        raise DependencyCycleError(f"proposed child dependencies are cyclic: {exc.args[1]}") from exc


class Scheduler(Service):
    def eligible(
        self, ctx: RequestContext, workspace_id: str | None = None, limit: int = 50
    ) -> list[dict[str, Any]]:
        max_attempts = self.app.config.execution.max_attempts_per_task
        with self.db.session() as session:
            rank = case({str(p): r for p, r in PRIORITY_RANK.items()}, value=Task.priority, else_=9)
            stmt = select(Task).where(
                Task.auto_execute.is_(True),
                Task.archived_at.is_(None),
                Task.status.in_([TaskStatus.READY, TaskStatus.FAILED]),
            )
            if workspace_id:
                stmt = stmt.where(Task.workspace_id == workspace_id)
            candidates = list(session.scalars(stmt.order_by(rank.asc(), Task.created_at.asc()).limit(500)))
            eligible: list[dict[str, Any]] = []
            for task in candidates:
                reasons = self._ineligibility(session, task, max_attempts)
                if reasons:
                    continue
                project = session.scalar(
                    select(TaskProject.project_id).where(TaskProject.task_id == task.id).limit(1)
                )
                eligible.append(
                    {
                        "task_id": task.id,
                        "key": task.key,
                        "priority": task.priority,
                        "project_id": project,
                        "agent": task.assigned_agent,
                        "workspace_id": task.workspace_id,
                    }
                )
            return self._fair_order(eligible)[:limit]

    def _ineligibility(self, session: Any, task: Task, max_attempts: int) -> list[str]:
        reasons = []
        if unsatisfied_dependencies(session, task.id):
            reasons.append("dependencies unsatisfied")
        active = session.scalar(
            select(func.count()).where(AgentAttempt.task_id == task.id, AgentAttempt.status.in_(ACTIVE))
        )
        if active:
            reasons.append("attempt already active")
        if task.status == TaskStatus.FAILED:
            failed = session.scalar(
                select(func.count()).where(
                    AgentAttempt.task_id == task.id,
                    AgentAttempt.parent_attempt_id.is_(None),
                    AgentAttempt.status.in_([AttemptStatus.FAILED, AttemptStatus.TIMED_OUT]),
                )
            )
            if (failed or 0) >= max_attempts or any(t.tag == "needs-review" for t in task.tags):
                reasons.append("retry budget exhausted (escalated for review)")
        for repo in session.scalars(
            select(Repository)
            .join(TaskRepository, TaskRepository.repository_id == Repository.id)
            .where(TaskRepository.task_id == task.id)
        ):
            if repo.archived_at is not None or primary_checkout(repo) is None:
                reasons.append(f"repository {repo.name} unavailable")
        return reasons

    def explain(self, ctx: RequestContext, workspace: str | None, task_ref: str) -> dict[str, Any]:
        from workgraph.core.services import resolve

        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            reasons = self._ineligibility(session, task, self.app.config.execution.max_attempts_per_task)
            if not task.auto_execute:
                reasons.insert(0, "auto_execute is off")
            if task.status not in (TaskStatus.READY, TaskStatus.FAILED):
                reasons.insert(0, f"status is {task.status}")
            if not self.app.config.execution.enabled:
                reasons.insert(0, "agent execution is disabled")
            return {"task": task.key, "eligible": not reasons, "reasons": reasons}

    @staticmethod
    def _fair_order(items: list[dict[str, Any]]) -> list[dict[str, Any]]:
        queues: OrderedDict[str, list[dict[str, Any]]] = OrderedDict()
        for item in items:  # already sorted by priority then age
            queues.setdefault(item["project_id"] or "-", []).append(item)
        ordered: list[dict[str, Any]] = []
        while any(queues.values()):
            best_rank = min(PRIORITY_RANK[q[0]["priority"]] for q in queues.values() if q)  # type: ignore[index]
            for queue in queues.values():
                if queue and PRIORITY_RANK[queue[0]["priority"]] == best_rank:  # type: ignore[index]
                    ordered.append(queue.pop(0))
        return ordered

    def tick(self, ctx: RequestContext) -> dict[str, Any]:
        config = self.app.config.execution
        if not config.enabled:
            return {"started": [], "reason": "execution disabled"}
        with self.db.session() as session:
            active = int(
                session.scalar(
                    select(func.count()).where(
                        AgentAttempt.status.in_(ACTIVE), AgentAttempt.parent_attempt_id.is_(None)
                    )
                )
                or 0
            )
        capacity = max(0, config.max_concurrent_runs - active)
        started: list[dict[str, Any]] = []
        errors: dict[str, str] = {}
        if capacity == 0:
            return {"started": [], "reason": "at capacity"}
        for item in self.eligible(ctx, limit=capacity * 3):
            if len(started) >= capacity:
                break
            try:
                attempt = self.app.runs.start(ctx, None, item["task_id"], agent=item["agent"])
                started.append({"task": item["key"], "attempt_id": attempt.id})
            except WorkgraphError as exc:
                errors[item["key"]] = exc.message
        return {"started": started, "errors": errors}

    # ------------------------------------------------------------------ decomposition

    def decompose(
        self,
        ctx: RequestContext,
        task_id: str,
        *,
        create: bool = True,
        provider: str | None = None,
        model: str | None = None,
    ) -> dict[str, Any]:
        """Ask the configured model for child tasks, validate them, and (optionally) create them."""
        with self.db.session() as session:
            task = session.get(Task, task_id)
            if task is None:
                raise NotFoundError(f"task {task_id} not found")
            slug = self.app.workspace_slug(task.workspace_id)
            repos = {
                r.name: r.id
                for r in session.scalars(
                    select(Repository)
                    .join(TaskRepository, TaskRepository.repository_id == Repository.id)
                    .where(TaskRepository.task_id == task.id)
                )
            }
            text = "\n".join(
                [
                    f"Title: {task.title}",
                    task.description,
                    *[f"- {c}" for c in task.acceptance_criteria or []],
                ]
            )
            key, ws_id = task.key, task.workspace_id
        selection = self.app.llm.selection("summarization", slug)
        llm = self.app.llm.get(provider or selection.provider, slug)
        prompt = f"Available repositories: {', '.join(repos) or '(none)'}\n\n<task>\n{text}\n</task>"
        schema = TaskDecomposition.model_json_schema()
        schema["title"] = "TaskDecomposition"
        response = llm.generate_json(
            model or selection.model, system=DECOMPOSE_SYSTEM, prompt=prompt, schema=schema
        )
        try:
            proposal = TaskDecomposition.model_validate(response.data)
        except ValidationError as exc:
            raise ValidationFailed(
                "model proposed an invalid decomposition", details={"errors": exc.errors()[:5]}
            ) from exc
        order = validate_decomposition(proposal, set(repos))
        result: dict[str, Any] = {
            "task": key,
            "proposal": proposal.model_dump(),
            "order": order,
            "provider": provider or selection.provider,
            "model": response.model,
            "created": [],
        }
        if not create:
            return result
        by_ref = {c.ref: c for c in proposal.children}
        created: dict[str, str] = {}
        with self.db.session(write=True) as session:
            for ref in order:
                child = by_ref[ref]
                out, _ = self.app.tasks._create_in_session(
                    session,
                    ctx,
                    ws_id,
                    TaskCreate(
                        title=child.title,
                        description=child.description,
                        parent=task_id,
                        repositories=child.repositories or list(repos),
                        depends_on=[created[d] for d in child.depends_on],
                        status="ready",
                        tags=["decomposed"],
                    ),
                )
                created[ref] = out.id
                result["created"].append(out.key)
            record_event(
                session,
                ctx,
                event_type="task.decomposed",
                entity_type="task",
                entity_id=task_id,
                workspace_id=ws_id,
                data={
                    "children": result["created"],
                    "provider": result["provider"],
                    "model": response.model,
                    "rationale": proposal.rationale[:1000],
                },
            )
        return result


def group_by(items: list[dict[str, Any]], key: str) -> dict[str, list[dict[str, Any]]]:
    grouped: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for item in items:
        grouped[str(item.get(key))].append(item)
    return grouped
