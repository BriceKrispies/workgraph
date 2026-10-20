"""Agent run orchestration: attempt lifecycle from queueing to verification.

``start`` validates permissions, opt-in and provider availability *before* any
state changes, creates a queued attempt and enqueues an ``agent_run`` job. The job
handler (:meth:`RunService.execute`) prepares isolated worktrees, runs the agent
outside any database transaction, captures the outcome, attributes commits, runs
verification and applies task/attempt transitions. Every failure is preserved.
"""

from __future__ import annotations

import builtins
import platform
import sys
import threading
import time
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING, Any

from pydantic import BaseModel
from sqlalchemy import func, select

from workgraph import __version__
from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import (
    InvalidTransitionError,
    NotFoundError,
    PermissionDenied,
    ProviderUnavailableError,
    ValidationFailed,
    WorkgraphError,
)
from workgraph.core.events.log import query_events, record_event
from workgraph.core.models.enums import TERMINAL_ATTEMPT_STATUSES, AttemptStatus, TaskStatus
from workgraph.core.models.schemas import AttemptOut, ClaimRequest
from workgraph.core.services import resolve
from workgraph.core.services.artifacts import store_bytes
from workgraph.core.services.attempts import finish_attempt, new_attempt, set_attempt_status
from workgraph.core.services.base import Service
from workgraph.integrations.agents.base import AgentEvent, AgentRunRequest, AgentRunResult
from workgraph.integrations.git.cli import Git
from workgraph.integrations.git.worktrees import RepoRequest
from workgraph.jobs import queue as jobqueue
from workgraph.orchestration.prompt import PROMPT_VERSION, build_prompt
from workgraph.persistence.orm import AgentAttempt, AgentSession, Job, RevisionManifest, Task, TaskTag
from workgraph.security.permissions import Permission, require

if TYPE_CHECKING:
    from workgraph.jobs.worker import JobContext

PROGRESS_EVENT_LIMIT = 500


class RunOut(BaseModel):
    attempt: AttemptOut
    job_id: str | None
    job_status: str | None
    recent_events: list[dict[str, Any]]


class StartedRepository(BaseModel):
    repository: str
    checkout_path: str
    branch: str | None
    base_sha: str | None
    base: dict[str, Any] | None


class AttemptStartOut(BaseModel):
    attempt_id: str
    attempt_number: int
    task_key: str
    repositories: list[StartedRepository]
    agent_checks: list[dict[str, Any]]


class RunService(Service):
    # ------------------------------------------------------------------ starting

    def _require_execution(self, ctx: RequestContext) -> None:
        require(ctx, Permission.RUN_START)
        if not self.app.config.execution.enabled:
            raise PermissionDenied(
                "agent code execution is disabled; opt in with `workgraph config set execution.enabled true`"
            )

    def start(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        *,
        agent: str | None = None,
        model: str | None = None,
        instructions: str | None = None,
        repositories: builtins.list[dict[str, str]] | None = None,
        retry_of: str | None = None,
        rerun_of: str | None = None,
        parent_attempt: str | None = None,
        priority: int = 50,
    ) -> AttemptOut:
        self._require_execution(ctx)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            slug = self.app.workspace_slug(task.workspace_id)
            agent_name = (
                agent or task.assigned_agent or self.app.effective_config(slug).execution.default_agent
            )
        provider = self.app.agents.require_available(agent_name, slug)
        with self.db.session(write=True) as session:
            task = resolve.task(session, None, task_ref if not ws_id else task.id)
            status = TaskStatus(task.status)
            if status in (TaskStatus.READY, TaskStatus.FAILED):
                self.app.tasks._claim_in_session(
                    session, ctx, task, ClaimRequest(agent=agent_name, lease_minutes=24 * 60)
                )
            elif status not in (TaskStatus.CLAIMED,):
                raise InvalidTransitionError(
                    f"task {task.key} is {status}; agent runs need a ready, failed or claimed task"
                )
            active = session.scalar(
                select(AgentAttempt).where(
                    AgentAttempt.task_id == task.id,
                    AgentAttempt.status.in_(
                        [
                            AttemptStatus.QUEUED,
                            AttemptStatus.PREPARING,
                            AttemptStatus.RUNNING,
                            AttemptStatus.VERIFYING,
                        ]
                    ),
                    AgentAttempt.parent_attempt_id.is_(None),
                )
            )
            if active is not None and parent_attempt is None:
                raise InvalidTransitionError(
                    f"task {task.key} already has an active attempt #{active.number}"
                )
            attempt = new_attempt(
                session,
                ctx,
                task,
                provider=agent_name,
                adapter=provider.adapter_type,
                status=AttemptStatus.QUEUED,
                agent_id=f"agent:{agent_name}",
                model=model,
                parent_attempt_id=parent_attempt,
                retry_of_attempt_id=retry_of,
                rerun_of_attempt_id=rerun_of,
                config_snapshot={
                    **provider.config_snapshot(),
                    "agent": agent_name,
                    "model": model,
                    "instructions": instructions,
                    "repositories": repositories,
                    "prompt_version": PROMPT_VERSION,
                    "isolation": self.app.config.execution.isolation,
                },
            )
            keys = ["run", f"agent:{agent_name}"]
            if self.app.config.execution.isolation == "in_place":
                keys += [f"exclusive:task-repos:{task.id}"]
            job = jobqueue.enqueue(
                session,
                "agent_run",
                {"attempt_id": attempt.id},
                workspace_id=task.workspace_id,
                priority=priority,
                task_id=task.id,
                attempt_id=attempt.id,
                max_attempts=1,
                resource_keys=keys,
                correlation_id=ctx.correlation_id,
                created_by=ctx.actor,
                timeout_seconds=None,
            )
            attempt.telemetry = {"job_id": job.id}
            session.flush()
            return AttemptOut.model_validate(attempt)

    def start_external(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        *,
        agent_name: str | None = None,
        adapter: str = "claude_code_subagent",
        model: str | None = None,
        isolation: str | None = None,
    ) -> AttemptStartOut:
        """Start an attempt an external agent works in, with the worktrees a managed run would get.

        Claims a ready task for the actor (or continues a claim the actor already holds), records an
        attempt, prepares its worktrees (a stack child gets its stack base) and moves task and attempt to
        running. The agent commits in the returned checkouts; finishing goes through the usual result calls.
        """
        require(ctx, Permission.TASK_CLAIM)
        require(ctx, Permission.ATTEMPT_WRITE)
        if isolation is not None and isolation not in ("worktree", "in_place"):
            raise ValidationFailed(f"isolation must be worktree or in_place, not {isolation!r}")
        agent = agent_name or ctx.actor.partition(":")[2] or ctx.actor
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            status = TaskStatus(task.status)
            if status == TaskStatus.READY:
                self.app.tasks._claim_in_session(session, ctx, task, ClaimRequest())
            elif status != TaskStatus.CLAIMED or task.claimed_by != ctx.actor:
                holder = f" by {task.claimed_by}" if status == TaskStatus.CLAIMED and task.claimed_by else ""
                raise ValidationFailed(
                    f"task {task.key} is {status}{holder}; an external attempt needs a ready task "
                    f"or one claimed by {ctx.actor}",
                    details={"status": str(status)},
                )
            active = session.scalar(
                select(AgentAttempt).where(
                    AgentAttempt.task_id == task.id,
                    AgentAttempt.status.in_(
                        [
                            AttemptStatus.QUEUED,
                            AttemptStatus.PREPARING,
                            AttemptStatus.RUNNING,
                            AttemptStatus.VERIFYING,
                        ]
                    ),
                    AgentAttempt.parent_attempt_id.is_(None),
                )
            )
            if active is not None:
                raise InvalidTransitionError(
                    f"task {task.key} already has an active attempt #{active.number}"
                )
            attempt = new_attempt(
                session,
                ctx,
                task,
                provider=agent,
                adapter=adapter,
                status=AttemptStatus.QUEUED,
                agent_id=ctx.actor,
                model=model,
                config_snapshot={"agent": agent, "model": model, "external": True},
            )
            set_attempt_status(session, ctx, attempt, AttemptStatus.PREPARING)
            attempt_id, number, task_id, task_key = attempt.id, attempt.number, task.id, task.key
        try:
            manifest = self.app.worktrees.prepare(ctx, attempt_id, isolation=isolation)
            with self.db.session(write=True) as session:
                attempt = resolve.attempt(session, attempt_id)
                task = session.get(Task, task_id)
                assert task is not None
                if task.status == TaskStatus.CLAIMED:
                    self.app.tasks._transition(
                        session, ctx, task, TaskStatus.RUNNING, reason=f"attempt #{number}"
                    )
                set_attempt_status(session, ctx, attempt, AttemptStatus.RUNNING)
        except Exception as exc:
            message = getattr(exc, "message", None) or str(exc)
            self._finalize_failure(
                ctx,
                attempt_id,
                AttemptStatus.FAILED,
                f"external attempt start failed: {message}",
                exit_reason="start_failed",
            )
            raise
        return AttemptStartOut(
            attempt_id=attempt_id,
            attempt_number=number,
            task_key=task_key,
            repositories=[
                StartedRepository(
                    repository=e.repository,
                    checkout_path=e.checkout_path,
                    branch=e.branch,
                    base_sha=e.sha,
                    base=e.base,
                )
                for e in manifest.repositories
            ],
            agent_checks=manifest.agent_checks,
        )

    def retry(self, ctx: RequestContext, attempt_id: str, *, instructions: str | None = None) -> AttemptOut:
        with self.db.session() as session:
            previous = resolve.attempt(session, attempt_id)
            if AttemptStatus(previous.status) not in TERMINAL_ATTEMPT_STATUSES:
                raise InvalidTransitionError("only finished attempts can be retried")
            if previous.status == AttemptStatus.SUCCEEDED:
                raise InvalidTransitionError("the attempt succeeded; use rerun to reproduce it")
            task_id, agent, model = previous.task_id, previous.provider, previous.model
        with self.db.session(write=True) as session:
            att = session.get(AgentAttempt, attempt_id)
            assert att is not None
            att.retries += 1
        return self.start(
            ctx, None, task_id, agent=agent, model=model, instructions=instructions, retry_of=attempt_id
        )

    def rerun(self, ctx: RequestContext, attempt_id: str) -> dict[str, Any]:
        """New attempt reusing the recorded configuration and baseline revisions where still available.

        Model execution is nondeterministic; a rerun reproduces the inputs, not the output.
        """
        with self.db.session() as session:
            previous = resolve.attempt(session, attempt_id)
            baseline = (
                session.get(RevisionManifest, previous.baseline_manifest_id)
                if previous.baseline_manifest_id
                else None
            )
            snapshot = dict(previous.config_snapshot or {})
            task_id = previous.task_id
            entries = list(baseline.entries) if baseline else []
        missing: list[dict[str, str]] = []
        repos: list[dict[str, str]] = []
        for entry in entries:
            source = entry.get("source_checkout_path") or entry.get("checkout_path")
            sha = entry.get("sha")
            if sha and source and Git(source).is_repository() and Git(source).object_exists(sha):
                repos.append(
                    {"repository_id": entry["repository_id"], "access": entry["access"], "target_ref": sha}
                )
            else:
                missing.append({"repository": entry.get("repository", "?"), "sha": str(sha)})
                repos.append({"repository_id": entry["repository_id"], "access": entry["access"]})
        attempt = self.start(
            ctx,
            None,
            task_id,
            agent=snapshot.get("agent"),
            model=snapshot.get("model"),
            instructions=snapshot.get("instructions"),
            repositories=repos or None,
            rerun_of=attempt_id,
        )
        return {
            "attempt": attempt.model_dump(mode="json"),
            "reproduced_baseline": not missing,
            "missing_revisions": missing,
            "note": "Agent/model execution is nondeterministic: inputs are reproduced, outputs may differ.",
        }

    def cancel(
        self, ctx: RequestContext, attempt_id: str, reason: str = "cancelled by operator"
    ) -> AttemptOut:
        require(ctx, Permission.RUN_CANCEL)
        with self.db.session(write=True) as session:
            attempt = resolve.attempt(session, attempt_id)
            if AttemptStatus(attempt.status) in TERMINAL_ATTEMPT_STATUSES:
                return AttemptOut.model_validate(attempt)
            attempt.cancel_requested = True
            job = session.scalar(select(Job).where(Job.attempt_id == attempt.id, Job.kind == "agent_run"))
            if attempt.status == AttemptStatus.QUEUED:
                if job is not None and job.status == "queued":
                    job.status = "cancelled"
                    job.cancel_requested = True
                    job.finished_at = utcnow()
                finish_attempt(
                    session, ctx, attempt, AttemptStatus.CANCELLED, exit_reason="cancelled", error=reason
                )
                task = session.get(Task, attempt.task_id)
                if task is not None and task.status in (TaskStatus.CLAIMED, TaskStatus.RUNNING):
                    self.app.tasks._transition(session, ctx, task, TaskStatus.READY, reason=reason)
            elif job is not None:
                job.cancel_requested = True
            record_event(
                session,
                ctx,
                event_type="attempt.cancel_requested",
                entity_type="attempt",
                entity_id=attempt.id,
                workspace_id=attempt.workspace_id,
                attempt_id=attempt.id,
                data={"reason": reason},
            )
            session.flush()
            return AttemptOut.model_validate(attempt)

    def get(self, ctx: RequestContext, attempt_id: str, *, events: int = 50) -> RunOut:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            attempt = resolve.attempt(session, attempt_id)
            job = session.scalar(select(Job).where(Job.attempt_id == attempt.id, Job.kind == "agent_run"))
            recent = query_events(session, attempt_id=attempt.id, limit=events, descending=True)
            return RunOut(
                attempt=AttemptOut.model_validate(attempt),
                job_id=job.id if job else None,
                job_status=job.status if job else None,
                recent_events=[e.to_dict() for e in reversed(recent)],
            )

    # ------------------------------------------------------------------ execution (job handler)

    def execute(self, job_ctx: JobContext) -> dict[str, Any]:
        attempt_id = job_ctx.job.payload["attempt_id"]
        ctx = RequestContext.system("runner").with_attempt(attempt_id)
        with self.db.session() as session:
            attempt = resolve.attempt(session, attempt_id)
            if AttemptStatus(attempt.status) in TERMINAL_ATTEMPT_STATUSES:
                return {"skipped": f"attempt already {attempt.status}"}
            task = session.get(Task, attempt.task_id)
            assert task is not None
            slug = self.app.workspace_slug(task.workspace_id)
            agent_name = attempt.provider
            snapshot = dict(attempt.input_task_snapshot or {})
            config_snapshot = dict(attempt.config_snapshot or {})
            task_id, ws_id = task.id, task.workspace_id
            cancel_flag = attempt.cancel_requested
        if cancel_flag:
            self._finalize_failure(ctx, attempt_id, AttemptStatus.CANCELLED, "cancelled before start")
            return {"status": "cancelled"}
        try:
            provider = self.app.agents.require_available(agent_name, slug)
        except (ProviderUnavailableError, NotFoundError) as exc:
            self._finalize_failure(
                ctx, attempt_id, AttemptStatus.FAILED, exc.message, exit_reason="provider_unavailable"
            )
            return {"status": "failed", "error": exc.message}

        with self.db.session(write=True) as session:
            attempt = resolve.attempt(session, attempt_id)
            set_attempt_status(session, ctx, attempt, AttemptStatus.PREPARING)
            task = session.get(Task, task_id)
            assert task is not None
            if task.status == TaskStatus.CLAIMED:
                self.app.tasks._transition(
                    session, ctx, task, TaskStatus.RUNNING, reason=f"attempt #{attempt.number}"
                )
        requests = None
        if config_snapshot.get("repositories"):
            requests = [
                RepoRequest(
                    repository_id=r["repository_id"], access=r["access"], target_ref=r.get("target_ref")
                )
                for r in config_snapshot["repositories"]
            ]
        try:
            manifest = self.app.worktrees.prepare(ctx, attempt_id, requests)
        except WorkgraphError as exc:
            self._finalize_failure(
                ctx,
                attempt_id,
                AttemptStatus.FAILED,
                f"workspace preparation failed: {exc.message}",
                exit_reason="workspace_preparation_failed",
            )
            return {"status": "failed", "error": exc.message}

        prompt = build_prompt(snapshot, manifest.model_dump(mode="json"), config_snapshot.get("instructions"))
        run_dir = self.app.paths.runs_dir / attempt_id
        run_dir.mkdir(parents=True, exist_ok=True)
        log_path = self.app.paths.artifacts_dir / ws_id / "runs" / attempt_id / "agent.log"
        writable = [e for e in manifest.repositories if e.access == "read_write"]
        workdir = Path(writable[0].checkout_path if writable else manifest.workspace_dir)
        others = [Path(e.checkout_path) for e in manifest.repositories if Path(e.checkout_path) != workdir]
        environment = {
            "platform": platform.platform(),
            "python": sys.version.split()[0],
            "workgraph": __version__,
            "provider_detail": provider.check_available()[1],
            "workdir": str(workdir),
            "workspace_dir": manifest.workspace_dir,
        }
        with self.db.session(write=True) as session:
            artifact = store_bytes(
                session,
                self.app.paths.artifacts_dir,
                workspace_id=ws_id,
                kind="prompt",
                name="prompt.md",
                content=prompt.encode("utf-8"),
                task_id=task_id,
                attempt_id=attempt_id,
                media_type="text/markdown",
                metadata={"prompt_version": PROMPT_VERSION},
            )
            attempt = resolve.attempt(session, attempt_id)
            attempt.prompt_artifact_id = artifact.id
            attempt.environment = environment
            set_attempt_status(session, ctx, attempt, AttemptStatus.RUNNING)

        request = AgentRunRequest(
            attempt_id=attempt_id,
            task_id=task_id,
            task_key=snapshot.get("key", ""),
            prompt=prompt,
            workdir=workdir,
            manifest_path=Path(manifest.workspace_dir) / "workgraph-manifest.json",
            manifest=manifest.model_dump(mode="json"),
            timeout_seconds=float(config_snapshot.get("timeout_seconds") or 3600),
            log_path=log_path,
            env={
                "WORKGRAPH_ATTEMPT_ID": attempt_id,
                "WORKGRAPH_TASK_ID": task_id,
                "WORKGRAPH_TASK_KEY": snapshot.get("key", ""),
                "WORKGRAPH_HOME": str(self.app.paths.config_dir.parent) if self._portable_home() else "",
                "WORKGRAPH_MANIFEST": str(Path(manifest.workspace_dir) / "workgraph-manifest.json"),
            },
            model=config_snapshot.get("model"),
            additional_dirs=others,
        )
        request.env = {k: v for k, v in request.env.items() if v}
        recorder = _EventRecorder(self, ctx, attempt_id, ws_id, agent_name, provider.adapter_type, task_id)
        cancel_checker = _CancelChecker(self, attempt_id, job_ctx)
        started = time.monotonic()
        try:
            result = provider.run(request, recorder, cancel_checker)
        except ProviderUnavailableError as exc:
            result = AgentRunResult(
                "failed", error=exc.message, telemetry={"exit_reason": "provider_unavailable"}
            )
        except Exception as exc:  # adapter crash: preserve it as a failed attempt
            result = AgentRunResult(
                "failed",
                error=f"adapter crashed: {type(exc).__name__}: {exc}",
                telemetry={"exit_reason": "adapter_crash"},
            )
        measured_ms = int((time.monotonic() - started) * 1000)
        recorder.flush()

        def record(session: Any) -> None:
            artifact = None
            if log_path.exists():
                from workgraph.core.services.artifacts import register_file

                artifact = register_file(
                    session,
                    self.app.paths.artifacts_dir,
                    log_path,
                    workspace_id=ws_id,
                    kind="agent_log",
                    task_id=task_id,
                    attempt_id=attempt_id,
                )
            attempt = resolve.attempt(session, attempt_id)
            attempt.session_id = result.session_id or attempt.session_id
            attempt.model = result.model or attempt.model
            telemetry = {
                **(attempt.telemetry or {}),
                **result.telemetry,
                "measured_duration_ms": measured_ms,
                "provider_reported_duration_ms": result.provider_duration_ms,
                "num_turns": result.num_turns,
                "agent_log_artifact_id": artifact.id if artifact else None,
                "artifacts": result.artifacts,
                "progress_events": recorder.count,
            }
            attempt.telemetry = telemetry

        usage_fields: dict[str, Any] = {}
        if result.usage:
            usage_fields = {"usage": result.usage, "usage_source": f"{provider.adapter_type}_reported"}
        if result.cost_usd is not None:
            usage_fields.update(cost_usd=result.cost_usd, cost_source=f"{provider.adapter_type}_reported")
        return self.conclude_attempt(
            ctx,
            task_id,
            attempt_id,
            status=result.status,
            summary=result.summary,
            error=result.error,
            exit_code=result.exit_code,
            duration_ms=measured_ms,
            should_cancel=job_ctx.cancel_requested,
            usage_fields=usage_fields,
            record=record,
        )

    def conclude_attempt(
        self,
        ctx: RequestContext,
        task_id: str,
        attempt_id: str,
        *,
        status: str,
        summary: str | None,
        error: str | None,
        exit_code: int | None,
        duration_ms: int | None,
        should_cancel: Callable[[], bool],
        usage_fields: dict[str, Any] | None = None,
        record: Callable[[Any], None] | None = None,
    ) -> dict[str, Any]:
        """Conclude an attempt the way every Workgraph run concludes.

        Captures the worktree outcome and links its commits, applies the read-only policy, then either fails the
        attempt and its task (with escalation) or hands the task to review or verification, and releases the
        worktrees. ``status`` is the agent's result status; ``record`` writes run-only records (telemetry, logs)
        in the same transaction as the attempt's outcome.
        """
        outcome: dict[str, Any] = {}
        # Without a captured outcome the attempt's commits are unknown, so it cannot count as a success:
        # a stack would treat the child as having changed nothing and merge it without its commits.
        capture_error: str | None = None
        try:
            outcome = self.app.worktrees.capture_outcome(ctx, attempt_id)
        except WorkgraphError as exc:
            capture_error = exc.message
            outcome = {"error": exc.message}
        if capture_error is None:
            try:
                self.app.git.link_attempt_commits(ctx, attempt_id)
            except WorkgraphError as exc:
                outcome = {**outcome, "link_error": exc.message}
        succeeded = status == "succeeded" and not outcome.get("violations") and capture_error is None
        with self.db.session(write=True) as session:
            if record is not None:
                record(session)
            attempt = resolve.attempt(session, attempt_id)
            if succeeded:
                for k, v in (usage_fields or {}).items():
                    setattr(attempt, k, v if k != "usage" else {**v})
                attempt.summary = summary[:50_000] if summary else attempt.summary
                attempt.exit_code = exit_code
                attempt.duration_ms = duration_ms
            else:
                attempt_status = {
                    "failed": AttemptStatus.FAILED,
                    "timed_out": AttemptStatus.TIMED_OUT,
                    "cancelled": AttemptStatus.CANCELLED,
                    "succeeded": AttemptStatus.FAILED,
                }[status]
                text = error or ""
                if outcome.get("violations"):
                    text = f"read-only repositories were modified: {outcome['violations']}. {text}".strip()
                if capture_error is not None:
                    text = f"could not record the attempt's changes: {capture_error}. {text}".strip()
                finish_attempt(
                    session,
                    ctx,
                    attempt,
                    attempt_status,
                    exit_reason=(
                        status
                        if status != "succeeded"
                        else "outcome_capture_failed"
                        if capture_error is not None
                        else "policy_violation"
                    ),
                    exit_code=exit_code,
                    error=text or "agent failed",
                    summary=summary,
                    duration_ms=duration_ms,
                    **(usage_fields or {}),
                )
                task = session.get(Task, task_id)
                assert task is not None
                target = TaskStatus.READY if attempt_status == AttemptStatus.CANCELLED else TaskStatus.FAILED
                if task.status in (TaskStatus.RUNNING, TaskStatus.CLAIMED):
                    self.app.tasks._transition(
                        session, ctx, task, target, reason=f"attempt #{attempt.number} {attempt_status}"
                    )
                self._maybe_escalate(session, ctx, task)
        if not succeeded:
            self.app.worktrees.release(ctx, attempt_id)
            return {"status": status, "error": error}

        # Agent claims success: that is not verification.
        with self.db.session(write=True) as session:
            task = session.get(Task, task_id)
            assert task is not None
            if task.status in (TaskStatus.RUNNING, TaskStatus.CLAIMED):
                self.app.tasks._complete_in_session(
                    session, ctx, task, summary=summary or "", attempt_id=attempt_id
                )
            attempt = resolve.attempt(session, attempt_id)
            needs_verification = task.status == TaskStatus.AWAITING_VERIFICATION
            if not needs_verification:
                finish_attempt(
                    session,
                    ctx,
                    attempt,
                    AttemptStatus.SUCCEEDED,
                    exit_reason="agent_succeeded",
                    exit_code=exit_code,
                    duration_ms=duration_ms,
                )
        verification: dict[str, Any] | None = None
        # A stack child is gated by its fast tier; the full tier runs once on the integration tip instead.
        gate = self.app.stacks.gate(task_id) if needs_verification else None
        if needs_verification and gate == "pass":
            with self.db.session(write=True) as session:
                attempt = resolve.attempt(session, attempt_id)
                finish_attempt(
                    session,
                    ctx,
                    attempt,
                    AttemptStatus.SUCCEEDED,
                    exit_reason="agent_succeeded",
                    exit_code=exit_code,
                    duration_ms=duration_ms,
                )
            self.app.stacks.mark_ready(ctx, task_id)
            verification = {"gate": "no_fast_checks"}
        elif needs_verification:
            try:
                run = self.app.verification.run(
                    ctx,
                    None,
                    task_id,
                    attempt_id=attempt_id,
                    should_cancel=should_cancel,
                    tier="fast" if gate == "fast" else None,
                )
                verification = {"run_id": run.id, "status": run.status}
                if gate == "fast" and run.status == "passed":
                    self.app.stacks.mark_ready(ctx, task_id)
            except WorkgraphError as exc:
                with self.db.session(write=True) as session:
                    attempt = resolve.attempt(session, attempt_id)
                    if AttemptStatus(attempt.status) not in TERMINAL_ATTEMPT_STATUSES:
                        finish_attempt(
                            session,
                            ctx,
                            attempt,
                            AttemptStatus.FAILED,
                            exit_reason="verification_error",
                            error=exc.message,
                        )
                verification = {"error": exc.message}
            with self.db.session(write=True) as session:
                task = session.get(Task, task_id)
                assert task is not None
                self._maybe_escalate(session, ctx, task)
        self.app.worktrees.release(ctx, attempt_id)
        return {
            "status": "succeeded",
            "verification": verification,
            "outcome": {k: v for k, v in outcome.items() if k != "commits"},
        }

    def abandon_external(self, ctx: RequestContext, attempt_ref: str, *, reason: str) -> dict[str, Any]:
        """Conclude an external attempt whose agent stopped without submitting.

        The attempt is marked abandoned with the reason, its task returns to ready (the claim is released
        with it) and its worktrees are released. Only attempts started with ``attempt start`` qualify; an
        orchestrated run is concluded by its own worker.
        """
        require(ctx, Permission.TASK_CLAIM)
        require(ctx, Permission.ATTEMPT_WRITE)
        if not reason.strip():
            raise ValidationFailed("abandoning an attempt needs a reason")
        with self.db.session(write=True) as session:
            attempt = resolve.attempt(session, attempt_ref)
            if not attempt.config_snapshot.get("external"):
                raise ValidationFailed(
                    f"attempt {attempt.id} was not started with attempt start; only external attempts are abandoned"
                )
            status = AttemptStatus(attempt.status)
            if status not in (AttemptStatus.RUNNING, AttemptStatus.PREPARING):
                raise ValidationFailed(
                    f"attempt {attempt.id} is {status}; only a running attempt can be abandoned"
                )
            task = session.get(Task, attempt.task_id)
            assert task is not None
            finish_attempt(
                session, ctx, attempt, AttemptStatus.ABANDONED, exit_reason="abandoned", error=reason
            )
            if TaskStatus(task.status) in (TaskStatus.CLAIMED, TaskStatus.RUNNING):
                self.app.tasks._transition(
                    session,
                    ctx,
                    task,
                    TaskStatus.READY,
                    reason=f"attempt #{attempt.number} abandoned: {reason}"[:200],
                )
            attempt_id, task_key = attempt.id, task.key
        self.app.worktrees.release(ctx, attempt_id)
        return {
            "attempt_id": attempt_id,
            "task": task_key,
            "attempt_status": "abandoned",
            "task_status": "ready",
        }

    def submit_external(
        self,
        ctx: RequestContext,
        attempt_ref: str,
        *,
        status: str = "succeeded",
        summary: str | None = None,
        error: str | None = None,
    ) -> dict[str, Any]:
        """Conclude an external managed attempt from the commits the agent made in its worktrees.

        Goes through :meth:`conclude_attempt`, the conclusion an orchestrated run gets, with the agent's own
        report as the result. Returns that conclusion plus the task's and the attempt's status afterwards.
        """
        require(ctx, Permission.TASK_CLAIM)
        require(ctx, Permission.ATTEMPT_WRITE)
        require(ctx, Permission.VERIFY_RUN)
        if status not in ("succeeded", "failed"):
            raise ValidationFailed(f"status must be succeeded or failed, not {status!r}")
        with self.db.session() as session:
            attempt = resolve.attempt(session, attempt_ref)
            attempt_id, task_id = attempt.id, attempt.task_id
            attempt_status = AttemptStatus(attempt.status)
            has_worktree = attempt.baseline_manifest_id is not None
            task = session.get(Task, task_id)
            assert task is not None
            task_key, task_status = task.key, TaskStatus(task.status)
        if attempt_status not in (AttemptStatus.RUNNING, AttemptStatus.PREPARING):
            raise ValidationFailed(
                f"attempt {attempt_id} is {attempt_status}; only a running attempt can be submitted"
            )
        if not has_worktree:
            raise ValidationFailed(
                f"attempt {attempt_id} has no managed worktree; submit applies to attempts from `attempt start`"
            )
        if task_status not in (TaskStatus.RUNNING, TaskStatus.CLAIMED):
            raise ValidationFailed(f"task {task_key} is {task_status}; only a running task can be submitted")
        if self.app.stacks.gate(task_id) is not None:
            require(ctx, Permission.STACK_WRITE)
        outcome = self.conclude_attempt(
            ctx.with_attempt(attempt_id),
            task_id,
            attempt_id,
            status=status,
            summary=summary,
            error=error,
            exit_code=None,
            duration_ms=None,
            should_cancel=lambda: False,
        )
        with self.db.session() as session:
            task = session.get(Task, task_id)
            attempt = resolve.attempt(session, attempt_id)
            assert task is not None
            return {**outcome, "task_status": str(task.status), "attempt_status": str(attempt.status)}

    def _portable_home(self) -> bool:
        import os

        return bool(os.environ.get("WORKGRAPH_HOME"))

    def _finalize_failure(
        self,
        ctx: RequestContext,
        attempt_id: str,
        status: AttemptStatus,
        error: str,
        *,
        exit_reason: str | None = None,
    ) -> None:
        with self.db.session(write=True) as session:
            attempt = resolve.attempt(session, attempt_id)
            if AttemptStatus(attempt.status) not in TERMINAL_ATTEMPT_STATUSES:
                finish_attempt(
                    session, ctx, attempt, status, exit_reason=exit_reason or str(status), error=error
                )
            task = session.get(Task, attempt.task_id)
            if task is not None and task.status in (TaskStatus.CLAIMED, TaskStatus.RUNNING):
                target = TaskStatus.READY if status == AttemptStatus.CANCELLED else TaskStatus.FAILED
                self.app.tasks._transition(session, ctx, task, target, reason=error[:200])
                self._maybe_escalate(session, ctx, task)
        self.app.worktrees.release(ctx, attempt_id)

    def _maybe_escalate(self, session: Any, ctx: RequestContext, task: Task) -> None:
        """After too many failed attempts, flag the task for human review instead of retrying."""
        if task.status != TaskStatus.FAILED:
            return
        failures = (
            session.scalar(
                select(func.count()).where(
                    AgentAttempt.task_id == task.id,
                    AgentAttempt.status.in_([AttemptStatus.FAILED, AttemptStatus.TIMED_OUT]),
                )
            )
            or 0
        )
        limit = self.app.config.execution.max_attempts_per_task
        if failures >= limit and not any(t.tag == "needs-review" for t in task.tags):
            task.tags.append(TaskTag(task_id=task.id, tag="needs-review"))
            record_event(
                session,
                ctx,
                event_type="task.escalated",
                entity_type="task",
                entity_id=task.id,
                workspace_id=task.workspace_id,
                data={"failed_attempts": failures, "limit": limit, "reason": "attempt limit reached"},
            )


class _EventRecorder:
    """Persists streamed agent events as attempt events (throttled) and session bindings."""

    def __init__(
        self,
        service: RunService,
        ctx: RequestContext,
        attempt_id: str,
        ws_id: str,
        provider: str,
        adapter: str,
        task_id: str,
    ) -> None:
        self.service = service
        self.ctx = ctx
        self.attempt_id = attempt_id
        self.ws_id = ws_id
        self.provider = provider
        self.adapter = adapter
        self.task_id = task_id
        self.count = 0
        self._last = 0.0
        self._pending: list[AgentEvent] = []
        self._lock = threading.Lock()

    def __call__(self, event: AgentEvent) -> None:
        with self._lock:
            if event.kind == "session" and event.data.get("session_id"):
                self._bind_session(event.data["session_id"], event.data.get("model"))
            if event.kind in ("log",) and self.count > PROGRESS_EVENT_LIMIT:
                return
            self._pending.append(event)
            now = time.monotonic()
            if event.kind in ("progress", "result", "session") or now - self._last > 1.0:
                self._write()

    def flush(self) -> None:
        with self._lock:
            self._write()

    def _write(self) -> None:
        if not self._pending or self.count >= PROGRESS_EVENT_LIMIT:
            self._pending.clear()
            return
        batch, self._pending = self._pending, []
        self._last = time.monotonic()
        with self.service.db.session(write=True) as session:
            for event in batch[-20:]:
                self.count += 1
                record_event(
                    session,
                    self.ctx,
                    event_type=f"attempt.{event.kind}",
                    entity_type="attempt",
                    entity_id=self.attempt_id,
                    workspace_id=self.ws_id,
                    attempt_id=self.attempt_id,
                    data={
                        "message": event.message[:2000],
                        **{
                            k: v
                            for k, v in event.data.items()
                            if isinstance(v, str | int | float | bool | type(None))
                        },
                    },
                )

    def _bind_session(self, session_id: str, model: str | None) -> None:
        provider_key = "claude_code" if self.adapter == "claude_code" else self.provider
        with self.service.db.session(write=True) as session:
            existing = session.scalar(
                select(AgentSession).where(
                    AgentSession.provider == provider_key, AgentSession.external_session_id == session_id
                )
            )
            if existing is None:
                session.add(
                    AgentSession(
                        provider=provider_key,
                        external_session_id=session_id,
                        workspace_id=self.ws_id,
                        attempt_id=self.attempt_id,
                        task_id=self.task_id,
                        binding_source="workgraph_launched",
                        model=model,
                        started_at=utcnow(),
                    )
                )
            else:
                existing.attempt_id = self.attempt_id
                existing.task_id = self.task_id
                existing.workspace_id = self.ws_id
                existing.binding_source = "workgraph_launched"
            attempt = session.get(AgentAttempt, self.attempt_id)
            if attempt is not None:
                attempt.session_id = session_id


class _CancelChecker:
    def __init__(self, service: RunService, attempt_id: str, job_ctx: JobContext) -> None:
        self.service = service
        self.attempt_id = attempt_id
        self.job_ctx = job_ctx
        self._last = 0.0
        self._cached = False

    def __call__(self) -> bool:
        if self._cached or self.job_ctx.cancel_requested():
            return True
        now = time.monotonic()
        if now - self._last > 2.0:
            self._last = now
            with self.service.db.session() as session:
                attempt = session.get(AgentAttempt, self.attempt_id)
                self._cached = bool(attempt and attempt.cancel_requested)
        return self._cached
