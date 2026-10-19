"""Verification runs and task acceptance.

A verification run executes the approved checks that apply to a task against an
exact, persisted revision manifest. Acceptance is the *only* way a task reaches
``accepted``: either automated (all required checks of the latest run passed) or an
explicitly recorded manual decision by an actor holding ``task:accept``.
"""

from __future__ import annotations

import json
import re
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

from pydantic import BaseModel, Field
from sqlalchemy import select
from sqlalchemy.orm import Session

from workgraph.core.clock import new_id, utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import (
    ConflictError,
    InvalidTransitionError,
    NotFoundError,
    ProviderUnavailableError,
    ValidationFailed,
)
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import (
    AttemptStatus,
    ChangeSetItemStatus,
    ChangeSetStatus,
    TaskStatus,
    VerificationStatus,
)
from workgraph.core.services import resolve
from workgraph.core.services.artifacts import register_file
from workgraph.core.services.base import Service
from workgraph.integrations.git.cli import Git
from workgraph.integrations.process import base_environment, run_process
from workgraph.persistence.orm import (
    AgentAttempt,
    ChangeSet,
    ChangeSetItem,
    Repository,
    RevisionManifest,
    Task,
    TaskAcceptance,
    TaskRepository,
    VerificationCheckResult,
    VerificationConfig,
    VerificationRun,
)
from workgraph.security.permissions import Permission, require
from workgraph.verification.configs import (
    ApplicableCheck,
    CheckSpec,
    CheckTier,
    applicable_checks,
    checks_for_tier,
)

_PYTEST_FAILED = re.compile(r"^(?:FAILED|ERROR) (\S+::\S+)")
_GENERIC_ERROR = re.compile(r"(?i)\b(error|failed|failure)\b[:\s](.{0,160})")
_CARGO_RUNNING = re.compile(r"^Running\s+.*\(([^()]+)\)$")
_CARGO_DOC_TESTS = re.compile(r"^Doc-tests\s+(\S+)$")
_CARGO_TEST_FAILED = re.compile(r"^test (.+) \.\.\. FAILED$")
_CARGO_BINARY_DIED = re.compile(r"^error: test failed, to rerun pass `")
_CARGO_BUILD_FAILED = re.compile(r"^error: could not compile `([^`]+)`")
_CARGO_TARGETS_FAILED = re.compile(r"^error: \d+ targets? failed:$")
_CARGO_TARGET_LINE = re.compile(r"^`([^`]+)`$")


class VerificationConfigOut(BaseModel):
    id: str
    workspace_id: str
    scope_type: str
    scope_id: str
    name: str
    checks: list[dict[str, Any]]
    enabled: bool
    approved_by: str
    version: int
    created_at: datetime
    updated_at: datetime

    model_config = {"from_attributes": True}


class CheckResultOut(BaseModel):
    id: str
    name: str
    kind: str
    required: bool
    repository_id: str | None
    status: str
    exit_code: int | None
    duration_ms: int | None
    timed_out: bool
    log_artifact_id: str | None
    output_excerpt: str
    failure_signatures: list[str]

    model_config = {"from_attributes": True}


class VerificationRunOut(BaseModel):
    id: str
    workspace_id: str
    task_id: str
    attempt_id: str | None
    manifest_id: str | None
    manifest: list[dict[str, Any]] = Field(default_factory=list)
    status: str
    required_passed: bool
    triggered_by: str
    started_at: datetime | None
    finished_at: datetime | None
    created_at: datetime
    checks: list[CheckResultOut] = Field(default_factory=list)

    model_config = {"from_attributes": True}


class AcceptanceOut(BaseModel):
    id: str
    task_id: str
    attempt_id: str | None
    verification_run_id: str | None
    decision: str
    mode: str
    reason: str
    actor: str
    manifest_id: str | None
    created_at: datetime

    model_config = {"from_attributes": True}


def _ran_fast_tier_only(run: VerificationRun, applicable: list[ApplicableCheck]) -> bool:
    """A run restricted to the fast tier does not cover the full tier, so it cannot accept a task."""
    if not any(check.spec.tier == "full" for check in applicable):
        return False
    return not any(entry.get("tier", "full") == "full" for entry in run.config_snapshot)


@dataclass
class _PreparedRun:
    run_id: str
    task_id: str
    task_key: str
    workspace_id: str
    attempt_id: str | None
    manifest: list[dict[str, Any]]
    checks: list[ApplicableCheck]
    tier: CheckTier | None = None
    event_context: dict[str, Any] = field(default_factory=dict)


def _cargo_binary_name(path: str) -> str:
    """``target\\debug\\deps\\rho_lexer-0a1b2c3d4e5f6789.exe`` -> ``rho_lexer``."""
    stem = re.split(r"[\\/]", path)[-1]
    stem = re.sub(r"\.exe$", "", stem)
    return re.sub(r"-[0-9a-f]+$", "", stem)


def _cargo_target_binary(target: str) -> str | None:
    """Binary name for one target in cargo's ``N targets failed`` summary.

    ``-p rho --test courtyard_gpu`` -> ``courtyard_gpu``, ``-p rho-lowering --lib`` -> ``rho_lowering``.
    Hyphens become underscores, matching the file names cargo uses under ``target/debug/deps``.
    """
    tokens = target.split()
    crate: str | None = None
    for i, word in enumerate(tokens[:-1]):
        if word == "-p":
            crate = tokens[i + 1]
        elif word in ("--test", "--bin", "--example", "--bench"):
            return tokens[i + 1].replace("-", "_")
    if "--lib" in tokens or "--doc" in tokens:
        return crate.replace("-", "_") if crate else None
    return None


def _cargo_signatures(lines: list[str]) -> list[str]:
    """Signatures from ``cargo test`` output, covering every test binary in the log.

    A failed test is ``test:<binary>::<name>`` (``test:<name>`` when no binary header was seen).
    A binary that reports ``error: test failed`` or appears in the ``N targets failed`` summary
    without any failed test of its own is ``binary:<binary>``. A crate that does not compile is
    ``build:<crate>``.
    """
    signatures: list[str] = []
    binary: str | None = None
    binaries_with_failures: set[str] = set()
    in_summary = False
    for raw in lines:
        line = raw.strip()
        target = _CARGO_TARGET_LINE.match(line)
        if in_summary and target:
            name = _cargo_target_binary(target.group(1))
            if name is not None and name not in binaries_with_failures:
                signatures.append(f"binary:{name}")
                binaries_with_failures.add(name)
            continue
        in_summary = _CARGO_TARGETS_FAILED.match(line) is not None
        if in_summary:
            continue
        if match := _CARGO_RUNNING.match(line):
            binary = _cargo_binary_name(match.group(1))
        elif match := _CARGO_DOC_TESTS.match(line):
            binary = match.group(1)
        elif match := _CARGO_TEST_FAILED.match(line):
            name = f"{binary}::{match.group(1)}" if binary else match.group(1)
            signatures.append(f"test:{name}")
            binaries_with_failures.add(binary or "")
        elif _CARGO_BINARY_DIED.match(line):
            if binary is not None and binary not in binaries_with_failures:
                signatures.append(f"binary:{binary}")
                binaries_with_failures.add(binary)
        elif match := _CARGO_BUILD_FAILED.match(line):
            signatures.append(f"build:{match.group(1)}")
    return list(dict.fromkeys(signatures))


def _failure_output(tail: list[str], log_path: Path) -> list[str]:
    """The whole check log when it can be read, otherwise the captured tail."""
    if log_path.exists():
        try:
            return log_path.read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            pass
    return tail


def extract_failure_signatures(lines: list[str], parser: str) -> list[str]:
    if parser in ("auto", "cargo"):
        cargo = _cargo_signatures(lines)
        if cargo or parser == "cargo":
            return cargo
    signatures: list[str] = []
    if parser in ("auto", "pytest"):
        for line in lines:
            match = _PYTEST_FAILED.match(line.strip())
            if match:
                signatures.append(f"test:{match.group(1)}")
    if not signatures and parser in ("auto", "generic"):
        for line in lines[-50:]:
            match = _GENERIC_ERROR.search(line)
            if match:
                signatures.append(f"error:{match.group(2).strip()[:120]}")
                if len(signatures) >= 5:
                    break
    return list(dict.fromkeys(signatures))


def manifest_from_checkouts(app: Any, session: Session, task: Task) -> list[dict[str, Any]]:
    """Entries describing the current HEAD of each task repository's primary checkout."""
    from workgraph.core.services.repositories import primary_checkout

    entries: list[dict[str, Any]] = []
    links = session.scalars(select(TaskRepository).where(TaskRepository.task_id == task.id))
    for link in links:
        repo = session.get(Repository, link.repository_id)
        if repo is None:
            continue
        checkout = primary_checkout(repo)
        entries.append(
            {
                "repository_id": repo.id,
                "repository": repo.name,
                "checkout_path": checkout.path if checkout else None,
                "sha": None,
                "ref": None,
                "access": link.access,
            }
        )
    return entries


def fill_heads(entries: list[dict[str, Any]]) -> list[dict[str, Any]]:
    for entry in entries:
        if entry.get("sha") is None and entry.get("checkout_path"):
            git = Git(entry["checkout_path"])
            if git.is_repository():
                entry["sha"] = git.head()
                entry["ref"] = git.current_branch()
    return entries


class VerificationService(Service):
    # ------------------------------------------------------------------ configuration (trusted)

    def create_config(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        scope_type: str,
        scope: str | None,
        name: str,
        checks: list[dict[str, Any]],
    ) -> VerificationConfigOut:
        require(ctx, Permission.CONFIG_MANAGE)
        specs = [CheckSpec.model_validate(c) for c in checks]
        if not specs:
            raise ValidationFailed("a verification configuration needs at least one check")
        names = [s.name for s in specs]
        if len(names) != len(set(names)):
            raise ValidationFailed("check names must be unique within a configuration")
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            scope_id = self._scope_id(session, ws_id, scope_type, scope)
            for spec in specs:
                if spec.repository:
                    resolve.repository(session, ws_id, spec.repository)
            existing = session.scalar(
                select(VerificationConfig).where(
                    VerificationConfig.workspace_id == ws_id,
                    VerificationConfig.scope_type == scope_type,
                    VerificationConfig.scope_id == scope_id,
                    VerificationConfig.name == name,
                )
            )
            payload = [s.model_dump(mode="json") for s in specs]
            if existing is None:
                config = VerificationConfig(
                    workspace_id=ws_id,
                    scope_type=scope_type,
                    scope_id=scope_id,
                    name=name,
                    checks=payload,
                    approved_by=ctx.actor,
                )
                session.add(config)
                event_type = "verification.config_created"
            else:
                config = existing
                config.checks = payload
                config.version += 1
                config.approved_by = ctx.actor
                config.enabled = True
                event_type = "verification.config_updated"
            session.flush()
            record_event(
                session,
                ctx,
                event_type=event_type,
                entity_type="verification_config",
                entity_id=config.id,
                workspace_id=ws_id,
                data={"scope_type": scope_type, "scope_id": scope_id, "name": name, "checks": payload},
            )
            return VerificationConfigOut.model_validate(config)

    def _scope_id(self, session: Session, ws_id: str, scope_type: str, scope: str | None) -> str:
        if scope_type == "workspace":
            return ws_id
        if scope is None:
            raise ValidationFailed(f"scope reference required for scope type {scope_type}")
        if scope_type == "project":
            return resolve.project(session, ws_id, scope).id
        if scope_type == "repository":
            return resolve.repository(session, ws_id, scope).id
        if scope_type == "task":
            return resolve.task(session, ws_id, scope).id
        raise ValidationFailed(f"unknown scope type {scope_type!r}")

    def set_config_enabled(self, ctx: RequestContext, config_id: str, enabled: bool) -> VerificationConfigOut:
        require(ctx, Permission.CONFIG_MANAGE)
        with self.db.session(write=True) as session:
            config = session.get(VerificationConfig, config_id)
            if config is None:
                raise NotFoundError(f"verification config {config_id} not found")
            config.enabled = enabled
            record_event(
                session,
                ctx,
                event_type="verification.config_enabled" if enabled else "verification.config_disabled",
                entity_type="verification_config",
                entity_id=config.id,
                workspace_id=config.workspace_id,
            )
            session.flush()
            return VerificationConfigOut.model_validate(config)

    def list_configs(self, ctx: RequestContext, workspace: str | None) -> list[VerificationConfigOut]:
        require(ctx, Permission.WORKSPACE_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            rows = session.scalars(
                select(VerificationConfig)
                .where(VerificationConfig.workspace_id == ws_id)
                .order_by(VerificationConfig.scope_type, VerificationConfig.name)
            )
            return [VerificationConfigOut.model_validate(r) for r in rows]

    def checks_for_task(
        self, ctx: RequestContext, workspace: str | None, task_ref: str
    ) -> list[dict[str, Any]]:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            task = resolve.task(session, ws_id, task_ref)
            return [
                {"config": c.config_name, "scope_type": c.scope_type, **c.spec.model_dump(mode="json")}
                for c in applicable_checks(session, task)
            ]

    # ------------------------------------------------------------------ runs

    def run(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        *,
        attempt_id: str | None = None,
        manifest_entries: list[dict[str, Any]] | None = None,
        should_cancel: Any = None,
        tier: CheckTier | None = None,
        check_filter: Callable[[ApplicableCheck], bool] | None = None,
        event_context: dict[str, Any] | None = None,
    ) -> VerificationRunOut:
        """Run the applicable checks; ``tier`` restricts the run to one tier (``None`` runs every tier).

        ``check_filter`` further narrows the checks, and ``event_context`` is merged into the run's events.
        """
        require(ctx, Permission.VERIFY_RUN)
        prepared = self._prepare(
            ctx, workspace, task_ref, attempt_id, manifest_entries, tier, check_filter, event_context or {}
        )
        results = [self._execute_check(prepared, check, should_cancel) for check in prepared.checks]
        return self._finalize(ctx, prepared, results)

    def _prepare(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        attempt_id: str | None,
        manifest_entries: list[dict[str, Any]] | None,
        tier: CheckTier | None,
        check_filter: Callable[[ApplicableCheck], bool] | None,
        event_context: dict[str, Any],
    ) -> _PreparedRun:
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            checks = checks_for_tier(applicable_checks(session, task), tier)
            if check_filter is not None:
                checks = [check for check in checks if check_filter(check)]
            if not checks:
                if tier is not None:
                    raise ValidationFailed(f"no {tier} verification checks are configured for {task.key}")
                raise ValidationFailed(
                    f"no verification checks are configured for {task.key}; record manual acceptance instead"
                )
            entries = manifest_entries
            if entries is None and attempt_id:
                attempt = resolve.attempt(session, attempt_id)
                manifest_id = attempt.outcome_manifest_id or attempt.baseline_manifest_id
                manifest = session.get(RevisionManifest, manifest_id) if manifest_id else None
                entries = [dict(e) for e in manifest.entries] if manifest else None
            if entries is None:
                entries = manifest_from_checkouts(self.app, session, task)
            task_id, task_key, task_ws = task.id, task.key, task.workspace_id
        entries = fill_heads([dict(e) for e in entries])
        with self.db.session(write=True) as session:
            manifest = RevisionManifest(
                workspace_id=task_ws, attempt_id=attempt_id, kind="verification", entries=entries
            )
            session.add(manifest)
            session.flush()
            run = VerificationRun(
                workspace_id=task_ws,
                task_id=task_id,
                attempt_id=attempt_id,
                manifest_id=manifest.id,
                config_snapshot=[
                    {"config_id": c.config_id, "config": c.config_name, **c.spec.model_dump(mode="json")}
                    for c in checks
                ],
                status=VerificationStatus.RUNNING,
                triggered_by=ctx.actor,
                started_at=utcnow(),
            )
            session.add(run)
            session.flush()
            if attempt_id:
                attempt = session.get(AgentAttempt, attempt_id)
                if attempt is not None and attempt.status == AttemptStatus.RUNNING:
                    from workgraph.core.services.attempts import set_attempt_status

                    set_attempt_status(session, ctx, attempt, AttemptStatus.VERIFYING)
            record_event(
                session,
                ctx,
                event_type="verification.started",
                entity_type="verification_run",
                entity_id=run.id,
                workspace_id=task_ws,
                attempt_id=attempt_id,
                data={
                    "task_id": task_id,
                    "task_key": task_key,
                    "manifest_id": manifest.id,
                    "checks": len(checks),
                    "tier": tier,
                    **event_context,
                },
            )
            return _PreparedRun(
                run_id=run.id,
                task_id=task_id,
                task_key=task_key,
                workspace_id=task_ws,
                attempt_id=attempt_id,
                manifest=entries,
                checks=checks,
                tier=tier,
                event_context=event_context,
            )

    def _execute_check(
        self, prepared: _PreparedRun, check: ApplicableCheck, should_cancel: Any
    ) -> dict[str, Any]:
        spec = check.spec
        run_dir = self.app.paths.runs_dir / "verification" / prepared.run_id
        run_dir.mkdir(parents=True, exist_ok=True)
        manifest_path = run_dir / "manifest.json"
        manifest_path.write_text(json.dumps(prepared.manifest, indent=2), encoding="utf-8")
        cwd: Path = run_dir
        repo_id: str | None = None
        if spec.repository:
            entry = next(
                (e for e in prepared.manifest if spec.repository in (e["repository_id"], e["repository"])),
                None,
            )
            if entry is None or not entry.get("checkout_path"):
                return {
                    "spec": spec,
                    "status": VerificationStatus.ERROR,
                    "exit_code": None,
                    "duration_ms": 0,
                    "timed_out": False,
                    "excerpt": f"repository {spec.repository} is not part of the revision manifest",
                    "signatures": ["error:repository missing from manifest"],
                    "log": None,
                    "repository_id": None,
                }
            cwd = Path(entry["checkout_path"])
            repo_id = entry["repository_id"]
        env_extra = {
            "WORKGRAPH_TASK_ID": prepared.task_id,
            "WORKGRAPH_TASK_KEY": prepared.task_key,
            "WORKGRAPH_VERIFICATION_RUN_ID": prepared.run_id,
            "WORKGRAPH_MANIFEST": str(manifest_path),
            **spec.env,
        }
        for entry in prepared.manifest:
            if entry.get("checkout_path"):
                key = re.sub(r"[^A-Z0-9]", "_", entry["repository"].upper())
                env_extra[f"WORKGRAPH_REPO_{key}_PATH"] = entry["checkout_path"]
        log_path = (
            self.app.paths.artifacts_dir
            / prepared.workspace_id
            / "verification"
            / prepared.run_id
            / (re.sub(r"[^A-Za-z0-9._-]", "_", spec.name) + ".log")
        )
        try:
            result = run_process(
                spec.command,
                cwd=cwd,
                env=base_environment(env_extra),
                timeout=spec.timeout_seconds,
                should_cancel=should_cancel,
                log_file=log_path,
            )
        except (ProviderUnavailableError, ValidationFailed) as exc:
            return {
                "spec": spec,
                "status": VerificationStatus.ERROR,
                "exit_code": None,
                "duration_ms": 0,
                "timed_out": False,
                "excerpt": exc.message,
                "signatures": [f"error:{exc.message[:120]}"],
                "log": None,
                "repository_id": repo_id,
            }
        if result.ok:
            status = VerificationStatus.PASSED
        elif result.cancelled:
            status = VerificationStatus.ERROR
        else:
            status = VerificationStatus.FAILED
        signatures = (
            []
            if result.ok
            else extract_failure_signatures(_failure_output(result.tail, log_path), spec.parser)
        )
        if result.timed_out:
            signatures.insert(0, "timeout")
        return {
            "spec": spec,
            "status": status,
            "exit_code": result.exit_code,
            "duration_ms": result.duration_ms,
            "timed_out": result.timed_out,
            "excerpt": "\n".join(result.tail[-40:]),
            "signatures": signatures,
            "log": log_path if log_path.exists() else None,
            "repository_id": repo_id,
        }

    def _finalize(
        self, ctx: RequestContext, prepared: _PreparedRun, results: list[dict[str, Any]]
    ) -> VerificationRunOut:
        with self.db.session(write=True) as session:
            run = session.get(VerificationRun, prepared.run_id)
            assert run is not None
            required_ok = True
            any_failed = False
            failed_names: list[str] = []
            for res in results:
                spec: CheckSpec = res["spec"]
                artifact_id = None
                if res["log"] is not None:
                    artifact = register_file(
                        session,
                        self.app.paths.artifacts_dir,
                        res["log"],
                        workspace_id=prepared.workspace_id,
                        kind="verification_log",
                        task_id=prepared.task_id,
                        attempt_id=prepared.attempt_id,
                        metadata={"check": spec.name, "run_id": prepared.run_id},
                    )
                    artifact_id = artifact.id
                session.add(
                    VerificationCheckResult(
                        run_id=run.id,
                        name=spec.name,
                        kind=spec.kind,
                        required=spec.required,
                        repository_id=res["repository_id"],
                        status=res["status"],
                        exit_code=res["exit_code"],
                        duration_ms=res["duration_ms"],
                        timed_out=res["timed_out"],
                        log_artifact_id=artifact_id,
                        output_excerpt=res["excerpt"][-8000:],
                        failure_signatures=res["signatures"],
                    )
                )
                if res["status"] != VerificationStatus.PASSED:
                    any_failed = True
                    failed_names.append(spec.name)
                    if spec.required:
                        required_ok = False
            run.status = VerificationStatus.PASSED if required_ok else VerificationStatus.FAILED
            run.required_passed = required_ok
            run.finished_at = utcnow()
            session.flush()
            event = record_event(
                session,
                ctx,
                event_type="verification.completed",
                entity_type="verification_run",
                entity_id=run.id,
                workspace_id=prepared.workspace_id,
                attempt_id=prepared.attempt_id,
                data={
                    "task_id": prepared.task_id,
                    "task_key": prepared.task_key,
                    "status": run.status,
                    "required_passed": required_ok,
                    "optional_failures": any_failed and required_ok,
                    "failed_checks": failed_names,
                    **prepared.event_context,
                },
            )
            follow = ctx.caused_by(event.id)
            # A fast-tier pass only gates a stack child (the stack item becomes ready); the change set
            # is verified, and the task moved on, by the full tier on the integration tip.
            fast_pass = prepared.tier == "fast" and required_ok
            if not fast_pass:
                self._apply_change_set_status(session, prepared, required_ok)
            if prepared.attempt_id:
                attempt = session.get(AgentAttempt, prepared.attempt_id)
                if attempt is not None and attempt.status == AttemptStatus.VERIFYING:
                    from workgraph.core.services.attempts import finish_attempt

                    finish_attempt(
                        session,
                        follow,
                        attempt,
                        AttemptStatus.SUCCEEDED if required_ok else AttemptStatus.FAILED,
                        exit_reason="verification_passed" if required_ok else "verification_failed",
                        error=None if required_ok else "verification failed: " + ", ".join(failed_names),
                    )
            task = session.get(Task, prepared.task_id)
            assert task is not None
            if task.status == TaskStatus.AWAITING_VERIFICATION and not fast_pass:
                target = TaskStatus.AWAITING_REVIEW if required_ok else TaskStatus.FAILED
                self.app.tasks._transition(
                    session,
                    follow,
                    task,
                    target,
                    reason=f"verification {run.status}",
                    extra={"run_id": run.id},
                )
            session.flush()
            return self._run_out(session, run)

    def _apply_change_set_status(self, session: Session, prepared: _PreparedRun, passed: bool) -> None:
        if not prepared.attempt_id:
            return
        change_set = session.scalar(select(ChangeSet).where(ChangeSet.attempt_id == prepared.attempt_id))
        if change_set is None:
            return
        items = list(
            session.scalars(select(ChangeSetItem).where(ChangeSetItem.change_set_id == change_set.id))
        )
        for item in items:
            if item.status == ChangeSetItemStatus.COMMITTED:
                item.status = ChangeSetItemStatus.VERIFIED if passed else ChangeSetItemStatus.FAILED
        statuses = {i.status for i in items}
        if passed and statuses <= {ChangeSetItemStatus.VERIFIED}:
            change_set.status = ChangeSetStatus.VERIFIED
        elif passed and ChangeSetItemStatus.VERIFIED in statuses:
            change_set.status = ChangeSetStatus.PARTIALLY_VERIFIED
        elif not passed:
            change_set.status = ChangeSetStatus.FAILED

    def _run_out(self, session: Session, run: VerificationRun) -> VerificationRunOut:
        out = VerificationRunOut.model_validate(run)
        manifest = session.get(RevisionManifest, run.manifest_id) if run.manifest_id else None
        out.manifest = list(manifest.entries) if manifest else []
        out.checks = [
            CheckResultOut.model_validate(c)
            for c in session.scalars(
                select(VerificationCheckResult)
                .where(VerificationCheckResult.run_id == run.id)
                .order_by(VerificationCheckResult.created_at)
            )
        ]
        return out

    def get_run(self, ctx: RequestContext, run_id: str) -> VerificationRunOut:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            run = session.get(VerificationRun, run_id)
            if run is None:
                raise NotFoundError(f"verification run {run_id} not found")
            return self._run_out(session, run)

    def list_runs(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        task: str | None = None,
        attempt_id: str | None = None,
        status: str | None = None,
        limit: int = 50,
    ) -> list[VerificationRunOut]:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            stmt = select(VerificationRun).where(VerificationRun.workspace_id == ws_id)
            if task:
                stmt = stmt.where(VerificationRun.task_id == resolve.task(session, ws_id, task).id)
            if attempt_id:
                stmt = stmt.where(VerificationRun.attempt_id == attempt_id)
            if status:
                stmt = stmt.where(VerificationRun.status == status)
            rows = session.scalars(stmt.order_by(VerificationRun.created_at.desc()).limit(limit))
            return [self._run_out(session, r) for r in rows]

    def failures(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        check_name: str | None = None,
        repository: str | None = None,
        since: datetime | None = None,
        limit: int = 200,
    ) -> list[dict[str, Any]]:
        """Independently queryable history of failed checks."""
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            stmt = (
                select(VerificationCheckResult, VerificationRun, Task)
                .join(VerificationRun, VerificationRun.id == VerificationCheckResult.run_id)
                .join(Task, Task.id == VerificationRun.task_id)
                .where(
                    VerificationRun.workspace_id == ws_id,
                    VerificationCheckResult.status.in_([VerificationStatus.FAILED, VerificationStatus.ERROR]),
                )
            )
            if check_name:
                stmt = stmt.where(VerificationCheckResult.name == check_name)
            if repository:
                stmt = stmt.where(
                    VerificationCheckResult.repository_id == resolve.repository(session, ws_id, repository).id
                )
            if since:
                stmt = stmt.where(VerificationCheckResult.created_at >= since)
            rows = session.execute(stmt.order_by(VerificationCheckResult.created_at.desc()).limit(limit))
            return [
                {
                    "check_result_id": c.id,
                    "run_id": r.id,
                    "task_id": t.id,
                    "task_key": t.key,
                    "attempt_id": r.attempt_id,
                    "check": c.name,
                    "kind": c.kind,
                    "required": c.required,
                    "status": c.status,
                    "exit_code": c.exit_code,
                    "timed_out": c.timed_out,
                    "failure_signatures": c.failure_signatures,
                    "repository_id": c.repository_id,
                    "created_at": c.created_at.isoformat(),
                }
                for c, r, t in rows
            ]

    # ------------------------------------------------------------------ acceptance

    def accept(
        self, ctx: RequestContext, workspace: str | None, task_ref: str, *, reason: str = ""
    ) -> AcceptanceOut:
        """Automated acceptance: requires the latest verification run to have passed.

        Accepting the parent of a stack lands the stack instead (see ``StackLander``).
        """
        require(ctx, Permission.TASK_ACCEPT)
        stack_id = self._stack_of_parent(workspace, task_ref)
        if stack_id is not None:
            return self._land_stack(
                ctx, stack_id, mode="automated", reason=reason or "required checks passed"
            )
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            checks = applicable_checks(session, task)
            if not checks:
                raise ValidationFailed(
                    f"{task.key} has no verification configured; use manual acceptance with a recorded reason"
                )
            latest = session.scalar(
                select(VerificationRun)
                .where(VerificationRun.task_id == task.id)
                .order_by(VerificationRun.created_at.desc())
                .limit(1)
            )
            if latest is None or latest.status == VerificationStatus.RUNNING:
                raise ConflictError(f"{task.key} has no completed verification run")
            if not latest.required_passed:
                raise ConflictError(
                    f"latest verification run for {task.key} failed required checks; it cannot be accepted",
                    details={"run_id": latest.id},
                )
            if _ran_fast_tier_only(latest, checks):
                raise ConflictError(
                    f"latest verification run for {task.key} ran only fast checks; run full verification first",
                    details={"run_id": latest.id},
                )
            self.app.stack_lander.guard_child(session, task)
            out = self._accept(
                session, ctx, task, mode="automated", reason=reason or "required checks passed", run=latest
            )
            rebuild = self.app.stack_lander.release_child(session, ctx, task)
        self.app.stack_lander.rebuild_without(ctx, rebuild)
        return out

    def accept_manually(
        self, ctx: RequestContext, workspace: str | None, task_ref: str, *, reason: str
    ) -> AcceptanceOut:
        require(ctx, Permission.TASK_ACCEPT)
        if not reason.strip():
            raise ValidationFailed("manual acceptance requires a recorded reason")
        stack_id = self._stack_of_parent(workspace, task_ref)
        if stack_id is not None:
            return self._land_stack(ctx, stack_id, mode="manual", reason=reason)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            latest = session.scalar(
                select(VerificationRun)
                .where(VerificationRun.task_id == task.id)
                .order_by(VerificationRun.created_at.desc())
                .limit(1)
            )
            self.app.stack_lander.guard_child(session, task)
            out = self._accept(session, ctx, task, mode="manual", reason=reason, run=latest)
            rebuild = self.app.stack_lander.release_child(session, ctx, task)
        self.app.stack_lander.rebuild_without(ctx, rebuild)
        return out

    def _stack_of_parent(self, workspace: str | None, task_ref: str) -> str | None:
        """The id of the task's active stack when the task is that stack's parent."""
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            stack = self.app.stack_lander.active_stack_of_parent(session, task.id)
            return stack.id if stack is not None else None

    def _land_stack(self, ctx: RequestContext, stack_id: str, *, mode: str, reason: str) -> AcceptanceOut:
        landed = self.app.stack_lander.land(ctx, stack_id, mode=mode, reason=reason)
        if landed is None:
            raise ConflictError(
                "the default branch moved since the stack was verified; the stack was rebuilt on the new "
                "branch and lands automatically once it verifies",
                details={"stack_id": stack_id, "landing": "deferred"},
            )
        return landed

    def _accept(
        self,
        session: Session,
        ctx: RequestContext,
        task: Task,
        *,
        mode: str,
        reason: str,
        run: VerificationRun | None,
    ) -> AcceptanceOut:
        if TaskStatus(task.status) not in (TaskStatus.AWAITING_REVIEW, TaskStatus.AWAITING_VERIFICATION):
            raise InvalidTransitionError(
                f"{task.key} is {task.status}; only completed work awaiting verification or review "
                "can be accepted"
            )
        latest_attempt = session.scalar(
            select(AgentAttempt)
            .where(AgentAttempt.task_id == task.id, AgentAttempt.status == AttemptStatus.SUCCEEDED)
            .order_by(AgentAttempt.number.desc())
            .limit(1)
        )
        manifest_id = None
        if run is not None and run.manifest_id:
            source = session.get(RevisionManifest, run.manifest_id)
            if source is not None:
                accepted = RevisionManifest(
                    workspace_id=task.workspace_id,
                    attempt_id=latest_attempt.id if latest_attempt else None,
                    kind="accepted",
                    entries=list(source.entries),
                )
                session.add(accepted)
                session.flush()
                manifest_id = accepted.id
        acceptance = TaskAcceptance(
            id=new_id(),
            task_id=task.id,
            attempt_id=latest_attempt.id if latest_attempt else None,
            verification_run_id=run.id if run else None,
            decision="accepted",
            mode=mode,
            reason=reason,
            actor=ctx.actor,
            manifest_id=manifest_id,
        )
        session.add(acceptance)
        session.flush()
        event = record_event(
            session,
            ctx,
            event_type="task.accepted",
            entity_type="task",
            entity_id=task.id,
            workspace_id=task.workspace_id,
            attempt_id=acceptance.attempt_id,
            data={
                "mode": mode,
                "reason": reason,
                "verification_run_id": acceptance.verification_run_id,
                "manifest_id": manifest_id,
            },
        )
        self.app.tasks._transition(
            session,
            ctx.caused_by(event.id),
            task,
            TaskStatus.ACCEPTED,
            privileged=True,
            reason=f"{mode} acceptance",
        )
        from workgraph.jobs import queue as jobqueue

        jobqueue.enqueue(
            session,
            "compute_change_surface",
            {"task_id": task.id},
            workspace_id=task.workspace_id,
            task_id=task.id,
            idempotency_key=f"surface:{task.id}:{acceptance.id}",
            causation_id=event.id,
            correlation_id=ctx.correlation_id,
        )
        session.flush()
        return AcceptanceOut.model_validate(acceptance)

    def reject(
        self, ctx: RequestContext, workspace: str | None, task_ref: str, *, reason: str
    ) -> AcceptanceOut:
        require(ctx, Permission.TASK_ACCEPT)
        if not reason.strip():
            raise ValidationFailed("rejection requires a reason")
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            if TaskStatus(task.status) not in (TaskStatus.AWAITING_REVIEW, TaskStatus.AWAITING_VERIFICATION):
                raise InvalidTransitionError(f"{task.key} is {task.status}; nothing to reject")
            acceptance = TaskAcceptance(
                task_id=task.id, decision="rejected", mode="manual", reason=reason, actor=ctx.actor
            )
            session.add(acceptance)
            session.flush()
            event = record_event(
                session,
                ctx,
                event_type="task.rejected",
                entity_type="task",
                entity_id=task.id,
                workspace_id=task.workspace_id,
                data={"reason": reason},
            )
            self.app.tasks._transition(
                session, ctx.caused_by(event.id), task, TaskStatus.READY, reason="rejected: " + reason
            )
            self.app.stack_lander.record_parent_rejected(session, ctx, task, reason)
            session.flush()
            return AcceptanceOut.model_validate(acceptance)

    def acceptances(self, ctx: RequestContext, workspace: str | None, task_ref: str) -> list[AcceptanceOut]:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            rows = session.scalars(
                select(TaskAcceptance)
                .where(TaskAcceptance.task_id == task.id)
                .order_by(TaskAcceptance.created_at)
            )
            return [AcceptanceOut.model_validate(r) for r in rows]
