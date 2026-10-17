"""Automation engine.

Flow: the elected maintenance worker reads new domain events after the
``automation`` cursor, matches enabled event rules, evaluates conditions
deterministically and records an ``automation_runs`` row per (rule, trigger) - the
unique constraint makes processing idempotent. Matched runs execute their actions
in an ``automation_action`` job with the rule's retry policy; already-completed
actions are skipped on retry.

Safety:

* Rules can only be authored by actors with ``automation:manage`` (humans by
  default); task text can be *interpolated* into titles/notifications but is never
  interpreted as rule logic or commands.
* Recursion is bounded: events produced by automation carry their depth, runs
  beyond the rule's ``max_depth`` (and the global limit) are skipped and recorded.
* A per-rule rate limit stops runaway loops that stay within depth.
* ``enqueue_agent_run`` additionally requires execution to be enabled and the rule's
  author to hold ``run:start``.
"""

from __future__ import annotations

import builtins
from dataclasses import replace
from datetime import datetime, timedelta
from typing import Any

from pydantic import BaseModel, TypeAdapter, ValidationError
from sqlalchemy import func, select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from workgraph.automation.cron import Cron
from workgraph.automation.rules import (
    PRIVILEGED_ACTIONS,
    TRIGGER_EVENTS,
    Action,
    Condition,
    RuleSpec,
    evaluate,
    render,
)
from workgraph.core.clock import new_id, utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import (
    ConflictError,
    NotFoundError,
    PermissionDenied,
    ValidationFailed,
    WorkgraphError,
)
from workgraph.core.events.log import EventRecord, record_event, to_record
from workgraph.core.models.enums import ActorKind
from workgraph.core.models.schemas import TaskCreate, TaskUpdate
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.jobs import queue as jobqueue
from workgraph.persistence.orm import (
    AgentAttempt,
    AutomationRule,
    AutomationRun,
    Classification,
    Event,
    Notification,
    SystemCursor,
    Task,
    TaskRepository,
)
from workgraph.security.permissions import Permission, require

CURSOR = "automation"
PRIORITY_FREE_ACTIONS = {
    "classify_task",
    "add_tags",
    "update_task",
    "create_child_tasks",
    "create_dependency",
    "enqueue_agent_run",
    "run_verification",
    "generate_report",
    "notify",
}
RATE_LIMIT_PER_MINUTE = 30
ACTIONS_ADAPTER: TypeAdapter[list[Action]] = TypeAdapter(list[Action])

AUTOMATION_PERMISSIONS = frozenset(
    {
        Permission.TASK_READ,
        Permission.TASK_WRITE,
        Permission.WORKSPACE_READ,
        Permission.REPO_READ,
        Permission.CLASSIFY_REQUEST,
        Permission.VERIFY_RUN,
        Permission.ANALYTICS_READ,
        Permission.GIT_READ,
    }
)


class RuleOut(BaseModel):
    id: str
    workspace_id: str
    name: str
    description: str
    enabled: bool
    trigger: dict[str, Any]
    conditions: dict[str, Any]
    actions: list[dict[str, Any]]
    retry_max_attempts: int
    max_depth: int
    last_scheduled_at: datetime | None
    next_run_at: datetime | None
    created_by: str
    version: int
    created_at: datetime
    updated_at: datetime

    model_config = {"from_attributes": True}


class RunOut(BaseModel):
    id: str
    rule_id: str
    workspace_id: str
    trigger_type: str
    trigger_key: str
    trigger_event_id: str | None
    depth: int
    dry_run: bool
    status: str
    matched: bool
    reason: str
    condition_trace: list[dict[str, Any]]
    action_results: list[dict[str, Any]]
    error: str | None
    attempts: int
    created_at: datetime
    finished_at: datetime | None

    model_config = {"from_attributes": True}


def _event_depth(event: EventRecord) -> int:
    meta = event.data.get("_automation") if isinstance(event.data, dict) else None
    return int(meta.get("depth", 0)) if isinstance(meta, dict) else 0


class AutomationService(Service):
    # ------------------------------------------------------------------ rule management

    def create_rule(
        self, ctx: RequestContext, workspace: str | None, spec: RuleSpec | dict[str, Any]
    ) -> RuleOut:
        require(ctx, Permission.AUTOMATION_MANAGE)
        rule_spec = spec if isinstance(spec, RuleSpec) else self._parse(spec)
        self._check_privileged(ctx, rule_spec)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            if session.scalar(
                select(AutomationRule).where(
                    AutomationRule.workspace_id == ws_id, AutomationRule.name == rule_spec.name
                )
            ):
                raise ConflictError(f"rule {rule_spec.name!r} already exists")
            rule = AutomationRule(
                workspace_id=ws_id,
                name=rule_spec.name,
                description=rule_spec.description,
                enabled=rule_spec.enabled,
                trigger=rule_spec.trigger.model_dump(mode="json", exclude_none=True),
                conditions=rule_spec.conditions.model_dump(mode="json", by_alias=True, exclude_none=True)
                if rule_spec.conditions
                else {},
                actions=[a.model_dump(mode="json", exclude_none=True) for a in rule_spec.actions],
                retry_max_attempts=rule_spec.retry_max_attempts,
                max_depth=min(rule_spec.max_depth, self.app.config.automation.max_depth),
                created_by=ctx.actor,
            )
            if rule_spec.trigger.type == "schedule":
                rule.next_run_at = self._next_time(rule, utcnow())
            session.add(rule)
            session.flush()
            record_event(
                session,
                ctx,
                event_type="automation.rule_created",
                entity_type="automation_rule",
                entity_id=rule.id,
                workspace_id=ws_id,
                data={"name": rule.name, "trigger": rule.trigger},
            )
            return RuleOut.model_validate(rule)

    def _parse(self, data: dict[str, Any]) -> RuleSpec:
        try:
            return RuleSpec.model_validate(data)
        except ValidationError as exc:
            raise ValidationFailed(
                "invalid automation rule", details={"errors": exc.errors(include_url=False)}
            ) from exc

    def _check_privileged(self, ctx: RequestContext, spec: RuleSpec) -> None:
        if any(a.type in PRIVILEGED_ACTIONS for a in spec.actions):
            require(ctx, Permission.RUN_START)

    def update_rule(
        self, ctx: RequestContext, workspace: str | None, ref: str, spec: dict[str, Any]
    ) -> RuleOut:
        require(ctx, Permission.AUTOMATION_MANAGE)
        rule_spec = self._parse(spec)
        self._check_privileged(ctx, rule_spec)
        with self.db.session(write=True) as session:
            rule = self._rule(session, workspace, ref)
            rule.name, rule.description, rule.enabled = (
                rule_spec.name,
                rule_spec.description,
                rule_spec.enabled,
            )
            rule.trigger = rule_spec.trigger.model_dump(mode="json", exclude_none=True)
            rule.conditions = (
                rule_spec.conditions.model_dump(mode="json", by_alias=True, exclude_none=True)
                if rule_spec.conditions
                else {}
            )
            rule.actions = [a.model_dump(mode="json", exclude_none=True) for a in rule_spec.actions]
            rule.retry_max_attempts = rule_spec.retry_max_attempts
            rule.max_depth = min(rule_spec.max_depth, self.app.config.automation.max_depth)
            rule.version += 1
            rule.created_by = ctx.actor  # authority now rests with the last editor
            if rule_spec.trigger.type == "schedule":
                rule.next_run_at = self._next_time(rule, utcnow())
            record_event(
                session,
                ctx,
                event_type="automation.rule_updated",
                entity_type="automation_rule",
                entity_id=rule.id,
                workspace_id=rule.workspace_id,
                data={"version": rule.version},
            )
            session.flush()
            return RuleOut.model_validate(rule)

    def set_enabled(self, ctx: RequestContext, workspace: str | None, ref: str, enabled: bool) -> RuleOut:
        require(ctx, Permission.AUTOMATION_MANAGE)
        with self.db.session(write=True) as session:
            rule = self._rule(session, workspace, ref)
            rule.enabled = enabled
            record_event(
                session,
                ctx,
                event_type="automation.rule_enabled" if enabled else "automation.rule_disabled",
                entity_type="automation_rule",
                entity_id=rule.id,
                workspace_id=rule.workspace_id,
            )
            session.flush()
            return RuleOut.model_validate(rule)

    def _rule(self, session: Session, workspace: str | None, ref: str) -> AutomationRule:
        ws_id = self.app.tasks._workspace_id(session, workspace)
        rule = session.get(AutomationRule, ref)
        if rule is None or rule.workspace_id != ws_id:
            rule = session.scalar(
                select(AutomationRule).where(AutomationRule.workspace_id == ws_id, AutomationRule.name == ref)
            )
        if rule is None:
            raise NotFoundError(f"automation rule {ref!r} not found")
        return rule

    def list_rules(self, ctx: RequestContext, workspace: str | None) -> builtins.list[RuleOut]:
        require(ctx, Permission.AUTOMATION_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            return [
                RuleOut.model_validate(r)
                for r in session.scalars(
                    select(AutomationRule)
                    .where(AutomationRule.workspace_id == ws_id)
                    .order_by(AutomationRule.name)
                )
            ]

    def get_rule(self, ctx: RequestContext, workspace: str | None, ref: str) -> RuleOut:
        require(ctx, Permission.AUTOMATION_READ)
        with self.db.session() as session:
            return RuleOut.model_validate(self._rule(session, workspace, ref))

    def runs(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        rule: str | None = None,
        status: str | None = None,
        limit: int = 100,
    ) -> builtins.list[RunOut]:
        require(ctx, Permission.AUTOMATION_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            stmt = select(AutomationRun).where(AutomationRun.workspace_id == ws_id)
            if rule:
                stmt = stmt.where(AutomationRun.rule_id == self._rule(session, workspace, rule).id)
            if status:
                stmt = stmt.where(AutomationRun.status == status)
            return [
                RunOut.model_validate(r)
                for r in session.scalars(stmt.order_by(AutomationRun.created_at.desc()).limit(limit))
            ]

    def catalog(self) -> dict[str, Any]:
        return {
            "triggers": TRIGGER_EVENTS,
            "schedule": ["cron", "interval_seconds"],
            "manual": True,
            "actions": sorted(PRIORITY_FREE_ACTIONS),
            "operators": [
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
            ],
        }

    # ------------------------------------------------------------------ context

    def _context(
        self, session: Session, event: EventRecord | None, extra: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        context: dict[str, Any] = {"now": utcnow().isoformat(), "input": extra or {}}
        task: Task | None = None
        if event is not None:
            context["event"] = {
                "type": event.event_type,
                "actor": event.actor,
                "actor_kind": event.actor_kind,
                "entity_type": event.entity_type,
                "entity_id": event.entity_id,
                "data": event.data,
            }
            task_id = event.entity_id if event.entity_type == "task" else event.data.get("task_id")
            if task_id:
                task = session.get(Task, task_id)
            if event.entity_type == "attempt":
                attempt = session.get(AgentAttempt, event.entity_id)
                if attempt is not None:
                    context["attempt"] = {
                        "id": attempt.id,
                        "status": attempt.status,
                        "provider": attempt.provider,
                        "model": attempt.model,
                        "number": attempt.number,
                        "exit_reason": attempt.exit_reason,
                    }
                    task = task or session.get(Task, attempt.task_id)
        if task is None and extra and extra.get("task"):
            try:
                task = resolve.task(session, None, str(extra["task"]))
            except WorkgraphError:
                task = None
        if task is not None:
            context["task"] = {
                "id": task.id,
                "key": task.key,
                "title": task.title,
                "status": task.status,
                "priority": task.priority,
                "category": task.category,
                "tags": [t.tag for t in task.tags],
                "assignee": task.assignee,
                "auto_execute": task.auto_execute,
                "parent_id": task.parent_id,
                "description": task.description[:2000],
            }
            if task.current_classification_id:
                cls = session.get(Classification, task.current_classification_id)
                if cls is not None:
                    context["classification"] = {
                        "category": cls.category,
                        "confidence": cls.confidence,
                        "domain": cls.domain,
                        "complexity": cls.complexity,
                        "source": cls.source,
                        "model": cls.model,
                    }
        return context

    # ------------------------------------------------------------------ event dispatch

    def process_events(self, ctx: RequestContext, batch: int = 500) -> dict[str, int]:
        stats = {"events": 0, "runs": 0, "matched": 0, "skipped": 0}
        with self.db.session(write=True) as session:
            cursor = session.get(SystemCursor, CURSOR)
            if cursor is None:
                cursor = SystemCursor(name=CURSOR, value=0)
                session.add(cursor)
                session.flush()
            rules = list(session.scalars(select(AutomationRule).where(AutomationRule.enabled.is_(True))))
            event_rules = [r for r in rules if r.trigger.get("type") == "event"]
            events = [
                to_record(e)
                for e in session.scalars(
                    select(Event).where(Event.seq > cursor.value).order_by(Event.seq).limit(batch)
                )
            ]
            if not events:
                return stats
            for event in events:
                stats["events"] += 1
                for rule in event_rules:
                    if rule.workspace_id != event.workspace_id or event.event_type not in rule.trigger.get(
                        "event_types", []
                    ):
                        continue
                    outcome = self._create_run(session, ctx, rule, "event", event.id, event)
                    if outcome:
                        stats["runs"] += 1
                        stats["matched"] += outcome == "matched"
                        stats["skipped"] += outcome == "skipped"
            cursor.value = events[-1].seq
        return stats

    def _create_run(
        self,
        session: Session,
        ctx: RequestContext,
        rule: AutomationRule,
        trigger_type: str,
        trigger_key: str,
        event: EventRecord | None,
        *,
        extra: dict[str, Any] | None = None,
        dry_run: bool = False,
    ) -> str | None:
        depth = (_event_depth(event) if event else 0) + 1
        run = AutomationRun(
            rule_id=rule.id,
            workspace_id=rule.workspace_id,
            trigger_type=trigger_type,
            trigger_key=trigger_key,
            trigger_event_id=event.id if event else None,
            depth=depth,
            dry_run=dry_run,
            correlation_id=event.correlation_id if event else ctx.correlation_id,
            inputs=extra or {},
        )
        if not dry_run:
            exists = session.scalar(
                select(AutomationRun.id).where(
                    AutomationRun.rule_id == rule.id, AutomationRun.trigger_key == trigger_key
                )
            )
            if exists:
                return None
        limit_depth = min(rule.max_depth, self.app.config.automation.max_depth)
        if depth > limit_depth:
            run.status, run.reason = (
                "skipped",
                f"automation depth {depth} exceeds limit {limit_depth} (recursion guard)",
            )
        elif not dry_run and self._rate_limited(session, rule):
            run.status, run.reason = "skipped", f"rate limit of {RATE_LIMIT_PER_MINUTE} runs/minute exceeded"
        else:
            trace: list[dict[str, Any]] = []
            context = self._context(session, event, extra)
            condition = Condition.model_validate(rule.conditions) if rule.conditions else None
            run.matched = evaluate(condition, context, trace)
            run.condition_trace = trace
            if not run.matched:
                run.status, run.reason = "not_matched", "conditions evaluated to false"
            elif dry_run:
                run.status = "previewed"
                run.reason = "dry run: actions not executed"
                run.action_results = [
                    {"action": a, "planned": self._describe(a, context)} for a in rule.actions
                ]
            else:
                run.status, run.reason = "queued", "conditions matched"
        session.add(run)
        try:
            session.flush()
        except IntegrityError:
            session.rollback()
            return None
        if run.status == "queued":
            jobqueue.enqueue(
                session,
                "automation_action",
                {"run_id": run.id},
                workspace_id=rule.workspace_id,
                max_attempts=rule.retry_max_attempts,
                backoff_base_seconds=5,
                idempotency_key=f"automation:{run.id}",
                correlation_id=run.correlation_id,
                causation_id=run.trigger_event_id,
            )
            return "matched"
        return "skipped" if run.status == "skipped" else ("previewed" if dry_run else "not_matched")

    def _rate_limited(self, session: Session, rule: AutomationRule) -> bool:
        recent = session.scalar(
            select(func.count()).where(
                AutomationRun.rule_id == rule.id,
                AutomationRun.dry_run.is_(False),
                AutomationRun.status.in_(["queued", "running", "succeeded", "failed"]),
                AutomationRun.created_at > utcnow() - timedelta(minutes=1),
            )
        )
        return int(recent or 0) >= RATE_LIMIT_PER_MINUTE

    def _describe(self, action: dict[str, Any], context: dict[str, Any]) -> str:
        task = (context.get("task") or {}).get("key", "the triggering task")
        kind = action["type"]
        descriptions = {
            "classify_task": f"request classification of {task}",
            "add_tags": f"add tags {action.get('tags')} to {task}",
            "update_task": f"update {task}: "
            + ", ".join(f"{k}={v}" for k, v in action.items() if k != "type"),
            "create_child_tasks": f"create {len(action.get('children', []))} child task(s) under {task}",
            "create_dependency": f"add dependency for {task}",
            "enqueue_agent_run": f"enqueue an agent run ({action.get('agent') or 'default agent'}) for {task}",
            "run_verification": f"run verification checks for {task}",
            "generate_report": f"generate a {action.get('period_days', 7)}-day retrospective",
            "notify": f"notify: {render(action.get('title', ''), context)}",
        }
        return descriptions.get(kind, kind)

    # ------------------------------------------------------------------ schedule / manual / preview

    def _next_time(self, rule: AutomationRule, after: datetime) -> datetime:
        if rule.trigger.get("cron"):
            return Cron(rule.trigger["cron"]).next_after(after)
        return after + timedelta(seconds=int(rule.trigger.get("interval_seconds") or 3600))

    def run_scheduled(self, ctx: RequestContext, now: datetime | None = None) -> int:
        now = now or utcnow()
        created = 0
        with self.db.session(write=True) as session:
            rules = session.scalars(select(AutomationRule).where(AutomationRule.enabled.is_(True)))
            for rule in list(rules):
                if rule.trigger.get("type") != "schedule":
                    continue
                if rule.next_run_at is None:
                    rule.next_run_at = self._next_time(rule, now)
                    continue
                if rule.next_run_at > now:
                    continue
                slot = rule.next_run_at.replace(microsecond=0).isoformat()
                if self._create_run(
                    session, ctx, rule, "schedule", f"schedule:{slot}", None, extra={"scheduled_for": slot}
                ):
                    created += 1
                rule.last_scheduled_at = now
                rule.next_run_at = self._next_time(rule, now)
        return created

    def trigger(
        self,
        ctx: RequestContext,
        workspace: str | None,
        ref: str,
        *,
        inputs: dict[str, Any] | None = None,
        key: str | None = None,
    ) -> RunOut:
        """Explicit manual trigger (allowed for agents with ``automation:trigger``)."""
        require(ctx, Permission.AUTOMATION_TRIGGER)
        with self.db.session(write=True) as session:
            rule = self._rule(session, workspace, ref)
            if not rule.enabled:
                raise ConflictError(f"rule {rule.name} is disabled")
            if rule.trigger.get("type") != "manual" and ctx.actor_kind != ActorKind.HUMAN:
                raise PermissionDenied("agents may only trigger rules whose trigger type is manual")
            trigger_key = f"manual:{key or new_id()}"
            self._create_run(
                session, ctx, rule, "manual", trigger_key, None, extra=self._clean_inputs(inputs)
            )
            run = session.scalar(
                select(AutomationRun).where(
                    AutomationRun.rule_id == rule.id, AutomationRun.trigger_key == trigger_key
                )
            )
            if run is None:
                raise ConflictError("this manual trigger key was already used")
            return RunOut.model_validate(run)

    @staticmethod
    def _clean_inputs(inputs: dict[str, Any] | None) -> dict[str, Any]:
        clean: dict[str, Any] = {}
        for key, value in (inputs or {}).items():
            if isinstance(value, str | int | float | bool) and len(str(key)) <= 64:
                clean[str(key)] = value if not isinstance(value, str) else value[:1000]
        return clean

    def preview(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        rule: str | None = None,
        spec: dict[str, Any] | None = None,
        event_id: str | None = None,
        inputs: dict[str, Any] | None = None,
    ) -> RunOut:
        """Dry run: evaluate a stored rule (or an unsaved spec) against an event or inputs."""
        require(ctx, Permission.AUTOMATION_READ)
        with self.db.session(write=True) as session:
            if rule:
                target = self._rule(session, workspace, rule)
            elif spec:
                parsed = self._parse(spec)
                ws_id = self.app.tasks._workspace_id(session, workspace)
                target = session.scalar(
                    select(AutomationRule).where(
                        AutomationRule.workspace_id == ws_id,
                        AutomationRule.name == f"[preview] {parsed.name}",
                    )
                )
                if target is None:
                    target = AutomationRule(
                        workspace_id=ws_id,
                        name=f"[preview] {parsed.name}",
                        enabled=False,
                        created_by=ctx.actor,
                        trigger={},
                        actions=[],
                    )
                    session.add(target)
                target.trigger = parsed.trigger.model_dump(mode="json", exclude_none=True)
                target.conditions = (
                    parsed.conditions.model_dump(mode="json", by_alias=True, exclude_none=True)
                    if parsed.conditions
                    else {}
                )
                target.actions = [a.model_dump(mode="json", exclude_none=True) for a in parsed.actions]
                target.enabled = False
                session.flush()
            else:
                raise ValidationFailed("pass a rule name/id or a rule spec to preview")
            event = None
            if event_id:
                row = session.scalar(select(Event).where(Event.id == event_id))
                if row is None:
                    raise NotFoundError(f"event {event_id} not found")
                event = to_record(row)
            key = f"dry:{new_id()}"
            self._create_run(
                session, ctx, target, "preview", key, event, extra=self._clean_inputs(inputs), dry_run=True
            )
            run = session.scalar(select(AutomationRun).where(AutomationRun.trigger_key == key))
            assert run is not None
            return RunOut.model_validate(run)

    # ------------------------------------------------------------------ execution (job)

    def execute_run(self, run_id: str) -> dict[str, Any]:
        with self.db.session(write=True) as session:
            run = session.get(AutomationRun, run_id)
            if run is None:
                raise NotFoundError(f"automation run {run_id} not found")
            if run.status in ("succeeded", "skipped", "not_matched"):
                return {"status": run.status}
            rule = session.get(AutomationRule, run.rule_id)
            assert rule is not None
            run.status = "running"
            run.attempts += 1
            event = None
            if run.trigger_event_id:
                row = session.scalar(select(Event).where(Event.id == run.trigger_event_id))
                event = to_record(row) if row else None
            context = self._context(session, event, dict(run.inputs or {}))
            actions = [dict(a) for a in rule.actions]
            done = {r["index"] for r in (run.action_results or []) if r.get("status") == "succeeded"}
            results = [r for r in (run.action_results or []) if r.get("status") == "succeeded"]
            permissions = AUTOMATION_PERMISSIONS
            if rule.created_by.startswith("human:"):
                permissions = permissions | {Permission.RUN_START}
            actx = RequestContext.automation(rule.name, frozenset(permissions), run.depth)
            actx = replace(
                actx,
                correlation_id=run.correlation_id or actx.correlation_id,
                causation_id=run.trigger_event_id,
            )
            rule_name, ws_id = rule.name, rule.workspace_id
        error = None
        parsed = ACTIONS_ADAPTER.validate_python(actions)
        for index, action in enumerate(parsed):
            if index in done:
                continue
            try:
                output = self._execute_action(actx, ws_id, action, context)
                results.append({"index": index, "type": action.type, "status": "succeeded", "output": output})
            except WorkgraphError as exc:
                results.append(
                    {"index": index, "type": action.type, "status": "failed", "error": exc.message}
                )
                error = f"action {index} ({action.type}) failed: {exc.message}"
                break
        with self.db.session(write=True) as session:
            run = session.get(AutomationRun, run_id)
            assert run is not None
            run.action_results = results
            run.status = "failed" if error else "succeeded"
            run.error = error
            run.finished_at = utcnow()
            record_event(
                session,
                actx,
                event_type="automation.run_completed",
                entity_type="automation_run",
                entity_id=run.id,
                workspace_id=ws_id,
                data={"rule": rule_name, "status": run.status, "actions": len(results), "error": error},
            )
        if error:
            raise WorkgraphError(error)
        return {"status": "succeeded", "actions": len(results)}

    def _target_task(self, ws_id: str, context: dict[str, Any]) -> str:
        task = context.get("task")
        if not task:
            raise ValidationFailed("this action needs a task in its trigger context")
        return task["id"]

    def _execute_action(
        self, ctx: RequestContext, ws_id: str, action: Any, context: dict[str, Any]
    ) -> dict[str, Any]:
        app = self.app
        slug = app.workspace_slug(ws_id)
        match action.type:
            case "classify_task":
                return app.classification.request(ctx, slug, self._target_task(ws_id, context))
            case "add_tags":
                out = app.tasks.update(
                    ctx,
                    slug,
                    self._target_task(ws_id, context),
                    TaskUpdate(add_tags=[render(t, context, 100) for t in action.tags]),
                )
                return {"task": out.key, "tags": out.tags}
            case "update_task":
                fields = {k: v for k, v in action.model_dump(exclude_none=True).items() if k != "type"}
                out = app.tasks.update(
                    ctx, slug, self._target_task(ws_id, context), TaskUpdate(**fields, reason="automation")
                )
                return {"task": out.key, "status": out.status}
            case "create_child_tasks":
                parent = self._target_task(ws_id, context)
                repos: list[str] = []
                if action.inherit_repositories:
                    with self.db.session() as session:
                        repos = list(
                            session.scalars(
                                select(TaskRepository.repository_id).where(TaskRepository.task_id == parent)
                            )
                        )
                created: list[str] = []
                for child in action.children:
                    out = app.tasks.create(
                        ctx,
                        slug,
                        TaskCreate(
                            title=render(child.title, context, 300),
                            description=render(child.description, context, 5000),
                            priority=child.priority or "medium",
                            tags=child.tags,
                            parent=parent,
                            repositories=repos,
                            depends_on=[created[-1]] if action.sequential and created else [],
                            status="ready",
                        ),
                    )
                    created.append(out.key)
                return {"created": created}
            case "create_dependency":
                task = self._target_task(ws_id, context)
                if action.depends_on:
                    out = app.tasks.add_dependency(ctx, slug, task, render(action.depends_on, context, 100))
                else:
                    out = app.tasks.add_dependency(ctx, slug, render(action.blocks or "", context, 100), task)
                return {"task": out.key, "depends_on": out.depends_on}
            case "enqueue_agent_run":
                require(ctx, Permission.RUN_START)
                attempt = app.runs.start(
                    ctx, slug, self._target_task(ws_id, context), agent=action.agent, model=action.model
                )
                return {"attempt_id": attempt.id}
            case "run_verification":
                task = self._target_task(ws_id, context)
                with self.db.session(write=True) as session:
                    job = jobqueue.enqueue(
                        session,
                        "verify",
                        {"task_id": task},
                        workspace_id=ws_id,
                        task_id=task,
                        max_attempts=1,
                        created_by=ctx.actor,
                    )
                return {"job_id": job.id}
            case "generate_report":
                with self.db.session(write=True) as session:
                    job = jobqueue.enqueue(
                        session,
                        "generate_report",
                        {"workspace": slug, "scope": {"period_days": action.period_days}},
                        workspace_id=ws_id,
                        max_attempts=2,
                        created_by=ctx.actor,
                    )
                return {"job_id": job.id}
            case "notify":
                with self.db.session(write=True) as session:
                    note = Notification(
                        workspace_id=ws_id,
                        level=action.level,
                        title=render(action.title, context, 300),
                        body=render(action.body, context, 5000),
                        source=ctx.actor,
                        data={"task": (context.get("task") or {}).get("key")},
                    )
                    session.add(note)
                    session.flush()
                    record_event(
                        session,
                        ctx,
                        event_type="notification.created",
                        entity_type="notification",
                        entity_id=note.id,
                        workspace_id=ws_id,
                        data={"title": note.title, "level": note.level},
                    )
                    return {"notification_id": note.id}
        raise ValidationFailed(f"unsupported action {action.type}")

    # ------------------------------------------------------------------ notifications

    def notifications(
        self, ctx: RequestContext, workspace: str | None, *, unread_only: bool = False, limit: int = 50
    ) -> builtins.list[dict[str, Any]]:
        require(ctx, Permission.WORKSPACE_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            stmt = select(Notification).where(Notification.workspace_id == ws_id)
            if unread_only:
                stmt = stmt.where(Notification.read_at.is_(None))
            return [
                {
                    "id": n.id,
                    "level": n.level,
                    "title": n.title,
                    "body": n.body,
                    "source": n.source,
                    "data": n.data,
                    "read_at": n.read_at.isoformat() if n.read_at else None,
                    "created_at": n.created_at.isoformat(),
                }
                for n in session.scalars(stmt.order_by(Notification.created_at.desc()).limit(limit))
            ]

    def mark_read(self, ctx: RequestContext, notification_id: str) -> None:
        with self.db.session(write=True) as session:
            note = session.get(Notification, notification_id)
            if note is None:
                raise NotFoundError("notification not found")
            note.read_at = utcnow()
