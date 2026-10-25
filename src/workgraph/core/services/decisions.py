"""Structured decisions: a question a task is waiting on, the options on offer and the answer.

A decision replaces the ``needs-decision`` tag plus prose. Requesting one tags the task
``needs-decision``; answering or withdrawing the last open decision on a task clears that tag (and
``parked`` and ``triage:needs-decision`` with it) so the task flows on. The answer is also written as a
task comment, which is where agents and the task history read it.

Every mutation appends a ``task.decision_*`` event in the same transaction, so the live UI refreshes.
"""

from __future__ import annotations

import builtins
from collections.abc import Iterable
from typing import Any, Literal

from sqlalchemy import case, func, select
from sqlalchemy.orm import Session

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError, ValidationFailed
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import PRIORITY_RANK, CommentKind, TaskStatus
from workgraph.core.models.schemas import (
    CommentCreate,
    DecisionOptionIn,
    DecisionOut,
    DecisionTaskOut,
    DecisionWithTaskOut,
    TaskUpdate,
)
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.core.services.tasks import PARKED_TAG
from workgraph.persistence.orm import Task, TaskDecision
from workgraph.security.permissions import Permission, require

NEEDS_DECISION_TAG = "needs-decision"
TRIAGE_NEEDS_DECISION_TAG = "triage:needs-decision"
# Dropped from a task once none of its decisions is open.
CLEARED_TAGS = (NEEDS_DECISION_TAG, PARKED_TAG, TRIAGE_NEEDS_DECISION_TAG)

DecisionFilter = Literal["open", "decided", "withdrawn", "all"]


def _letter(position: int) -> str:
    """ "a", "b", ... "z" by position; a decision carries at most 26 options."""
    return chr(ord("a") + position)


def normalize_options(options: builtins.list[DecisionOptionIn]) -> builtins.list[dict[str, Any]]:
    """Fill in missing option ids from position and refuse duplicates."""
    out: builtins.list[dict[str, Any]] = []
    seen: set[str] = set()
    for position, option in enumerate(options):
        option_id = option.id or _letter(position)
        if option_id in seen:
            raise ValidationFailed(f"duplicate option id {option_id!r}")
        seen.add(option_id)
        out.append(
            {
                "id": option_id,
                "label": option.label,
                "detail": option.detail,
                "recommended": option.recommended,
            }
        )
    return out


def _out(task: Task, decision: TaskDecision) -> DecisionOut:
    return DecisionOut.model_validate({**_columns(decision), "task_key": task.key})


def _columns(decision: TaskDecision) -> dict[str, Any]:
    return {c.key: getattr(decision, c.key) for c in TaskDecision.__mapper__.column_attrs}


def _with_task(task: Task, decision: TaskDecision) -> DecisionWithTaskOut:
    brief = DecisionTaskOut(
        key=task.key,
        title=task.title,
        status=task.status,
        priority=task.priority,
        tags=[t.tag for t in task.tags],
        description=task.description,
    )
    return DecisionWithTaskOut.model_validate({**_out(task, decision).model_dump(), "task": brief})


def _answer_text(decision: TaskDecision) -> str:
    if decision.other_text is not None:
        return decision.other_text
    for option in decision.options:
        if option["id"] == decision.chosen_option_id:
            return option["label"]
    return str(decision.chosen_option_id)


class DecisionService(Service):
    # ------------------------------------------------------------------ internal helpers

    def _load(self, session: Session, workspace: str | None, decision_id: str) -> tuple[Task, TaskDecision]:
        decision = session.get(TaskDecision, decision_id)
        ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
        if decision is None or (ws_id is not None and decision.workspace_id != ws_id):
            raise NotFoundError(f"decision {decision_id!r} not found", details={"ref": decision_id})
        task = session.get(Task, decision.task_id)
        assert task is not None  # the foreign key cascades
        return task, decision

    def _event(
        self, session: Session, ctx: RequestContext, event_type: str, task: Task, decision: TaskDecision
    ) -> None:
        record_event(
            session,
            ctx,
            event_type=event_type,
            entity_type="task",
            entity_id=task.id,
            workspace_id=task.workspace_id,
            data={
                "key": task.key,
                "decision_id": decision.id,
                "question": decision.question[:500],
                "version": decision.version,
            },
        )

    def _tag(self, session: Session, ctx: RequestContext, task: Task, update: TaskUpdate) -> None:
        self.app.tasks._update_in_session(session, ctx, task, update, enrich=False)

    def _release_task(self, session: Session, ctx: RequestContext, task: Task) -> None:
        """Drop the waiting tags once no decision on the task is open."""
        still_open = session.scalar(
            select(func.count())
            .select_from(TaskDecision)
            .where(TaskDecision.task_id == task.id, TaskDecision.status == "open")
        )
        if still_open:
            return
        present = [tag for tag in CLEARED_TAGS if tag in [t.tag for t in task.tags]]
        if present:
            self._tag(session, ctx, task, TaskUpdate(remove_tags=present))

    def _comment(self, session: Session, ctx: RequestContext, task: Task, body: str) -> None:
        self.app.tasks._comment(session, ctx, task, CommentCreate(body=body, kind=CommentKind.COMMENT))

    # ------------------------------------------------------------------ request

    def request(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        question: str,
        options: Iterable[DecisionOptionIn | dict[str, Any]] = (),
    ) -> DecisionOut:
        """Open a decision on a task and tag the task ``needs-decision``. No options means a free-text answer."""
        require(ctx, Permission.TASK_WRITE)
        if not question.strip():
            raise ValidationFailed("question must not be empty")
        normalized = normalize_options([DecisionOptionIn.model_validate(o) for o in options])
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            decision = TaskDecision(
                workspace_id=task.workspace_id,
                task_id=task.id,
                question=question.strip(),
                options=normalized,
                status="open",
                created_by=ctx.actor,
            )
            session.add(decision)
            session.flush()
            self._event(session, ctx, "task.decision_requested", task, decision)
            if NEEDS_DECISION_TAG not in [t.tag for t in task.tags]:
                self._tag(session, ctx, task, TaskUpdate(add_tags=[NEEDS_DECISION_TAG]))
            session.flush()
            return _out(task, decision)

    # ------------------------------------------------------------------ read

    def list(
        self,
        ctx: RequestContext,
        workspace: str | None,
        status: DecisionFilter = "open",
        limit: int = 200,
    ) -> builtins.list[DecisionWithTaskOut]:
        """Decisions with their task. Open ones: task priority, then oldest first; archived or cancelled tasks drop out."""
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            stmt = (
                select(TaskDecision, Task)
                .join(Task, Task.id == TaskDecision.task_id)
                .where(TaskDecision.workspace_id == ws_id)
            )
            if status != "all":
                stmt = stmt.where(TaskDecision.status == status)
            if status == "open":
                stmt = stmt.where(Task.archived_at.is_(None), Task.status != TaskStatus.CANCELLED)
                rank = case({str(p): r for p, r in PRIORITY_RANK.items()}, value=Task.priority, else_=9)
                stmt = stmt.order_by(rank.asc(), TaskDecision.created_at.asc())
            elif status == "decided":
                stmt = stmt.order_by(TaskDecision.decided_at.desc())
            else:
                stmt = stmt.order_by(TaskDecision.created_at.desc())
            rows = session.execute(stmt.limit(limit)).all()
            return [_with_task(task, decision) for decision, task in rows]

    def get(self, ctx: RequestContext, workspace: str | None, decision_id: str) -> DecisionWithTaskOut:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            task, decision = self._load(session, workspace, decision_id)
            return _with_task(task, decision)

    def for_task(
        self, ctx: RequestContext, workspace: str | None, task_ref: str
    ) -> builtins.list[DecisionOut]:
        """Every decision on a task, oldest first."""
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            rows = session.scalars(
                select(TaskDecision)
                .where(TaskDecision.task_id == task.id)
                .order_by(TaskDecision.created_at.asc(), TaskDecision.id)
            )
            return [_out(task, d) for d in rows]

    # ------------------------------------------------------------------ answer

    def choose(
        self,
        ctx: RequestContext,
        workspace: str | None,
        decision_id: str,
        option_id: str | None = None,
        other_text: str | None = None,
        note: str | None = None,
    ) -> DecisionOut:
        """Answer with an option or free text. An answered decision may be answered again."""
        require(ctx, Permission.TASK_WRITE)
        if (option_id is None) == (other_text is None):
            raise ValidationFailed("give exactly one of option_id and other_text")
        if other_text is not None and not other_text.strip():
            raise ValidationFailed("other_text must not be empty")
        with self.db.session(write=True) as session:
            task, decision = self._load(session, workspace, decision_id)
            if decision.status == "withdrawn":
                raise ValidationFailed("decision was withdrawn; request a new one")
            if option_id is not None and option_id not in [o["id"] for o in decision.options]:
                known = ", ".join(o["id"] for o in decision.options) or "none, answer with other_text"
                raise ValidationFailed(
                    f"unknown option {option_id!r}; options are: {known}",
                    details={"option_id": option_id},
                )
            decision.status = "decided"
            decision.chosen_option_id = option_id
            decision.other_text = other_text.strip() if other_text is not None else None
            decision.note = note.strip() if note and note.strip() else None
            decision.decided_by = ctx.actor
            decision.decided_at = utcnow()
            decision.version += 1
            session.flush()
            body = f"User decision {decision.decided_at:%Y-%m-%d}: {decision.question} -> {_answer_text(decision)}"
            if decision.note:
                body += f"\n\nNote: {decision.note}"
            self._comment(session, ctx, task, body)
            self._event(session, ctx, "task.decision_chosen", task, decision)
            self._release_task(session, ctx, task)
            session.flush()
            return _out(task, decision)

    def withdraw(
        self, ctx: RequestContext, workspace: str | None, decision_id: str, reason: str | None = None
    ) -> DecisionOut:
        """Retire an open decision that no longer needs an answer."""
        require(ctx, Permission.TASK_WRITE)
        with self.db.session(write=True) as session:
            task, decision = self._load(session, workspace, decision_id)
            if decision.status != "open":
                raise ValidationFailed(
                    f"only an open decision can be withdrawn; this one is {decision.status}"
                )
            decision.status = "withdrawn"
            decision.version += 1
            session.flush()
            body = f"Decision withdrawn: {decision.question}"
            if reason:
                body += f"\n\nReason: {reason}"
            self._comment(session, ctx, task, body)
            self._event(session, ctx, "task.decision_withdrawn", task, decision)
            self._release_task(session, ctx, task)
            session.flush()
            return _out(task, decision)
