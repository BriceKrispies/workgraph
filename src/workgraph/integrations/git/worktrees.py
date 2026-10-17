"""Managed Git worktrees, execution manifests and cross-repository change sets.

Ownership rules:

* Workgraph only ever creates, modifies or removes worktrees **it created**, located
  under its own data directory and recorded in ``managed_worktrees``.
* User checkouts are never reset, cleaned or deleted. When an attempt must write in
  place (``execution.isolation = "in_place"``), an exclusive resource lease
  prevents two attempts from mutating the same checkout simultaneously.
* Worktree branches are never deleted automatically: they hold the agent's commits.
* Git cannot commit atomically across repositories, so a change set tracks each
  repository's state independently (pending / committed / verified / failed).
"""

from __future__ import annotations

import builtins
import re
import shutil
from dataclasses import dataclass, replace
from datetime import timedelta
from pathlib import Path
from typing import Any

from pydantic import BaseModel, Field
from sqlalchemy import delete, select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import GitError, NotFoundError, ResourceBusyError, ValidationFailed
from workgraph.core.events.log import record_event
from workgraph.core.fsutil import is_within
from workgraph.core.models.enums import (
    TERMINAL_ATTEMPT_STATUSES,
    AttemptStatus,
    ChangeSetItemStatus,
    ChangeSetStatus,
    WorktreeStatus,
)
from workgraph.core.services.base import Service
from workgraph.core.services.repositories import primary_checkout
from workgraph.core.services.stacks import stack_base
from workgraph.integrations.git.cli import Git, validate_ref
from workgraph.persistence.orm import (
    AgentAttempt,
    ChangeSet,
    ChangeSetItem,
    ManagedWorktree,
    Repository,
    ResourceLease,
    RevisionManifest,
    Task,
    TaskRepository,
)


class ManifestEntry(BaseModel):
    repository_id: str
    repository: str
    checkout_path: str
    source_checkout_path: str
    sha: str | None
    ref: str | None
    target_ref: str | None
    access: str
    isolation: str  # worktree | in_place | snapshot
    worktree_id: str | None = None
    branch: str | None = None
    # Why this base was chosen when a stack decided it (see core.services.stacks.stack_base).
    base: dict[str, Any] | None = None


class ExecutionManifest(BaseModel):
    """Everything an agent needs to know about the workspace it runs in."""

    task_id: str
    task_key: str
    attempt_id: str
    workspace_dir: str
    repositories: list[ManifestEntry]
    expected_changes: list[str]
    verification: list[dict[str, Any]]
    agent_checks: list[dict[str, Any]] = Field(default_factory=list)
    baseline_manifest_id: str
    change_set_id: str | None


@dataclass
class RepoRequest:
    repository_id: str
    access: str
    target_ref: str | None = None
    base: dict[str, Any] | None = None


def acquire_lease(
    session: Session, key: str, holder: str, *, purpose: str = "", ttl_seconds: int = 6 * 3600
) -> None:
    now = utcnow()
    existing = session.get(ResourceLease, key)
    if existing is not None:
        if existing.holder == holder:
            existing.expires_at = now + timedelta(seconds=ttl_seconds)
            return
        if existing.expires_at is None or existing.expires_at > now:
            raise ResourceBusyError(
                f"resource {key} is leased by {existing.holder}",
                details={"resource": key, "holder": existing.holder, "purpose": existing.purpose},
            )
        session.delete(existing)
        session.flush()
    session.add(
        ResourceLease(
            resource_key=key, holder=holder, purpose=purpose, expires_at=now + timedelta(seconds=ttl_seconds)
        )
    )
    try:
        session.flush()
    except IntegrityError as exc:
        raise ResourceBusyError(f"resource {key} was leased concurrently") from exc


def release_leases(session: Session, holder: str) -> int:
    result = session.execute(delete(ResourceLease).where(ResourceLease.holder == holder))
    return int(getattr(result, "rowcount", 0) or 0)


def _branch_name(task_key: str, attempt_number: int, attempt_id: str) -> str:
    slug = re.sub(r"[^a-z0-9-]", "-", task_key.lower())
    return f"wg/{slug}/attempt-{attempt_number}-{attempt_id[:8]}"


class WorktreeManager(Service):
    @property
    def root(self) -> Path:
        return self.app.paths.worktrees_dir

    # ------------------------------------------------------------------ preparation

    def default_requests(self, task_id: str) -> builtins.list[RepoRequest]:
        with self.db.session() as session:
            links = list(session.scalars(select(TaskRepository).where(TaskRepository.task_id == task_id)))
            requests = []
            for link in links:
                repo = session.get(Repository, link.repository_id)
                if repo is None or repo.archived_at is not None:
                    continue
                access = "read_only" if repo.access_mode == "read_only" else link.access
                requests.append(RepoRequest(repository_id=repo.id, access=access))
            return requests

    def prepare(
        self,
        ctx: RequestContext,
        attempt_id: str,
        requests: builtins.list[RepoRequest] | None = None,
        *,
        isolation: str | None = None,
        expected_changes: builtins.list[str] | None = None,
    ) -> ExecutionManifest:
        isolation = isolation or self.app.config.execution.isolation
        with self.db.session() as session:
            attempt = session.get(AgentAttempt, attempt_id)
            if attempt is None:
                raise NotFoundError(f"attempt {attempt_id} not found")
            task = session.get(Task, attempt.task_id)
            assert task is not None
            task_id, task_key, ws_id, number = task.id, task.key, task.workspace_id, attempt.number
        requests = requests if requests is not None else self.default_requests(task_id)
        if isolation != "in_place":
            requests = [self._stack_request(task_id, request) for request in requests]
        workspace_dir = self.root / attempt_id
        workspace_dir.mkdir(parents=True, exist_ok=True)
        entries: list[ManifestEntry] = []
        try:
            for request in requests:
                entries.append(
                    self._prepare_repo(
                        ctx, ws_id, attempt_id, task_key, number, request, isolation, workspace_dir
                    )
                )
        except Exception:
            self.release(ctx, attempt_id, remove=True)
            raise
        with self.db.session(write=True) as session:
            baseline = RevisionManifest(
                workspace_id=ws_id,
                attempt_id=attempt_id,
                kind="baseline",
                entries=[e.model_dump() for e in entries],
            )
            session.add(baseline)
            session.flush()
            change_set_id = None
            writable = [e for e in entries if e.access == "read_write"]
            if writable:
                change_set = ChangeSet(
                    workspace_id=ws_id,
                    task_id=task_id,
                    attempt_id=attempt_id,
                    title=f"{task_key} attempt {number}",
                )
                session.add(change_set)
                session.flush()
                for e in writable:
                    session.add(
                        ChangeSetItem(
                            change_set_id=change_set.id,
                            repository_id=e.repository_id,
                            base_sha=e.sha,
                            branch=e.branch,
                            status=ChangeSetItemStatus.PENDING,
                        )
                    )
                change_set_id = change_set.id
            attempt = session.get(AgentAttempt, attempt_id)
            assert attempt is not None
            attempt.baseline_manifest_id = baseline.id
            from workgraph.verification.configs import applicable_checks, checks_for_tier

            task_row = session.get(Task, task_id)
            assert task_row is not None
            applicable = applicable_checks(session, task_row)
            checks = [c.spec.model_dump(mode="json") for c in applicable]
            agent_checks = [
                {"name": c.spec.name, "command": c.spec.command, "repository": c.spec.repository}
                for c in checks_for_tier(applicable, "fast")
            ]
            record_event(
                session,
                ctx,
                event_type="attempt.workspace_prepared",
                entity_type="attempt",
                entity_id=attempt_id,
                workspace_id=ws_id,
                attempt_id=attempt_id,
                data={
                    "repositories": [e.repository for e in entries],
                    "bases": {e.repository: e.base for e in entries if e.base is not None},
                    "isolation": isolation,
                    "baseline_manifest_id": baseline.id,
                    "change_set_id": change_set_id,
                },
            )
            manifest = ExecutionManifest(
                task_id=task_id,
                task_key=task_key,
                attempt_id=attempt_id,
                workspace_dir=str(workspace_dir),
                repositories=entries,
                expected_changes=expected_changes or [e.repository for e in writable],
                verification=checks,
                agent_checks=agent_checks,
                baseline_manifest_id=baseline.id,
                change_set_id=change_set_id,
            )
        (workspace_dir / "workgraph-manifest.json").write_text(
            manifest.model_dump_json(indent=2), encoding="utf-8"
        )
        return manifest

    def _stack_request(self, task_id: str, request: RepoRequest) -> RepoRequest:
        """Give a stack child's request the base its dependencies imply, unless the caller chose one."""
        if request.target_ref is not None:
            return request
        with self.db.session() as session:
            repo = session.get(Repository, request.repository_id)
            task = session.get(Task, task_id)
            if repo is None or task is None:
                return request
            base = stack_base(session, task, repo)
        if base is None:
            return request
        return replace(request, target_ref=base.commit, base=base.info)

    def _prepare_repo(
        self,
        ctx: RequestContext,
        ws_id: str,
        attempt_id: str,
        task_key: str,
        attempt_number: int,
        request: RepoRequest,
        isolation: str,
        workspace_dir: Path,
    ) -> ManifestEntry:
        with self.db.session() as session:
            repo = session.get(Repository, request.repository_id)
            if repo is None or repo.workspace_id != ws_id:
                raise NotFoundError(f"repository {request.repository_id} not found")
            if request.access == "read_write" and repo.access_mode == "read_only":
                raise ValidationFailed(f"repository {repo.name} is read-only")
            checkout = primary_checkout(repo)
            if checkout is None:
                raise GitError(f"repository {repo.name} has no available checkout")
            name, checkout_id, source = repo.name, checkout.id, checkout.path
            default_branch = repo.default_branch
        git = Git(source)
        target = request.target_ref or default_branch
        if target:
            validate_ref(target)
            try:
                base = git.resolve_commit(target)
            except GitError:
                base = git.head()
        else:
            base = git.head()
        if base is None:
            raise GitError(f"repository {name} has no commits to base an attempt on")
        if isolation == "in_place" and request.access == "read_write":
            with self.db.session(write=True) as session:
                acquire_lease(
                    session, f"checkout:{checkout_id}", attempt_id, purpose=f"in-place write {task_key}"
                )
            return ManifestEntry(
                repository_id=request.repository_id,
                repository=name,
                checkout_path=source,
                source_checkout_path=source,
                sha=git.head(),
                ref=git.current_branch(),
                target_ref=target,
                access=request.access,
                isolation="in_place",
            )
        path = workspace_dir / re.sub(r"[^A-Za-z0-9._-]", "_", name)
        branch = (
            _branch_name(task_key, attempt_number, attempt_id) if request.access == "read_write" else None
        )
        with self.db.session(write=True) as session:
            row = ManagedWorktree(
                workspace_id=ws_id,
                repository_id=request.repository_id,
                source_checkout_id=checkout_id,
                attempt_id=attempt_id,
                path=str(path),
                branch=branch or "(detached)",
                base_sha=base,
                status=WorktreeStatus.PREPARING,
            )
            session.add(row)
            session.flush()
            worktree_id = row.id
        try:
            if branch:
                git.run("worktree", "add", "-b", branch, str(path), base)
            else:
                git.run("worktree", "add", "--detach", str(path), base)
        except GitError as exc:
            with self.db.session(write=True) as session:
                row = session.get(ManagedWorktree, worktree_id)
                if row is not None:
                    row.status = WorktreeStatus.ORPHANED
                    row.last_error = exc.message
            raise
        with self.db.session(write=True) as session:
            row = session.get(ManagedWorktree, worktree_id)
            assert row is not None
            row.status = WorktreeStatus.ACTIVE
            record_event(
                session,
                ctx,
                event_type="worktree.created",
                entity_type="worktree",
                entity_id=worktree_id,
                workspace_id=ws_id,
                attempt_id=attempt_id,
                data={
                    "repository": name,
                    "path": str(path),
                    "branch": branch,
                    "base_sha": base,
                    "base": request.base,
                },
            )
        return ManifestEntry(
            repository_id=request.repository_id,
            repository=name,
            checkout_path=str(path),
            source_checkout_path=source,
            sha=base,
            ref=branch,
            target_ref=target,
            access=request.access,
            isolation="worktree" if branch else "snapshot",
            worktree_id=worktree_id,
            branch=branch,
            base=request.base,
        )

    # ------------------------------------------------------------------ outcome

    def capture_outcome(self, ctx: RequestContext, attempt_id: str) -> dict[str, Any]:
        """Record the outcome manifest and update the change set; enforce read-only access."""
        with self.db.session() as session:
            attempt = session.get(AgentAttempt, attempt_id)
            if attempt is None or attempt.baseline_manifest_id is None:
                raise NotFoundError(f"attempt {attempt_id} has no prepared workspace")
            baseline = session.get(RevisionManifest, attempt.baseline_manifest_id)
            assert baseline is not None
            entries = [ManifestEntry.model_validate(e) for e in baseline.entries]
            ws_id = attempt.workspace_id
        outcome: list[dict[str, Any]] = []
        violations: list[dict[str, Any]] = []
        modified: list[str] = []
        commits_by_repo: dict[str, list[str]] = {}
        dirty: dict[str, list[str]] = {}
        for entry in entries:
            git = Git(entry.checkout_path)
            head = git.head() if git.is_repository() else None
            status = git.status_porcelain() if head else None
            changed_files = (status.staged + status.unstaged + status.untracked) if status else []
            data = entry.model_dump()
            data["sha"] = head
            outcome.append(data)
            commits: list[str] = []
            if head and entry.sha and head != entry.sha:
                commits = git.out("rev-list", "--reverse", f"{entry.sha}..{head}").split()
            commits_by_repo[entry.repository_id] = commits
            if changed_files:
                dirty[entry.repository] = changed_files[:50]
            if entry.access == "read_only" and (commits or changed_files):
                violations.append(
                    {
                        "repository": entry.repository,
                        "commits": len(commits),
                        "uncommitted_files": len(changed_files),
                    }
                )
            elif commits:
                modified.append(entry.repository_id)
        with self.db.session(write=True) as session:
            manifest = RevisionManifest(
                workspace_id=ws_id, attempt_id=attempt_id, kind="outcome", entries=outcome
            )
            session.add(manifest)
            session.flush()
            attempt = session.get(AgentAttempt, attempt_id)
            assert attempt is not None
            attempt.outcome_manifest_id = manifest.id
            attempt.modified_repository_ids = modified
            telemetry = dict(attempt.telemetry or {})
            telemetry["uncommitted_changes"] = dirty
            if violations:
                telemetry["read_only_violations"] = violations
            attempt.telemetry = telemetry
            change_set = session.scalar(select(ChangeSet).where(ChangeSet.attempt_id == attempt_id))
            if change_set is not None:
                items = list(
                    session.scalars(select(ChangeSetItem).where(ChangeSetItem.change_set_id == change_set.id))
                )
                head_by_repo = {e["repository_id"]: e["sha"] for e in outcome}
                for item in items:
                    shas = commits_by_repo.get(item.repository_id, [])
                    item.head_sha = head_by_repo.get(item.repository_id)
                    item.commit_shas = shas
                    item.status = ChangeSetItemStatus.COMMITTED if shas else ChangeSetItemStatus.PENDING
                    item.detail = None if shas else "no commits produced"
                committed = sum(1 for i in items if i.status == ChangeSetItemStatus.COMMITTED)
                if committed == 0:
                    change_set.status = ChangeSetStatus.OPEN
                elif committed < len(items):
                    change_set.status = ChangeSetStatus.PARTIALLY_APPLIED
                else:
                    change_set.status = ChangeSetStatus.APPLIED
            record_event(
                session,
                ctx,
                event_type="attempt.outcome_captured",
                entity_type="attempt",
                entity_id=attempt_id,
                workspace_id=ws_id,
                attempt_id=attempt_id,
                data={
                    "outcome_manifest_id": manifest.id,
                    "modified_repositories": modified,
                    "read_only_violations": violations,
                    "commits": {k: len(v) for k, v in commits_by_repo.items()},
                },
            )
            return {
                "outcome_manifest_id": manifest.id,
                "modified": modified,
                "violations": violations,
                "commits": commits_by_repo,
                "uncommitted": dirty,
            }

    # ------------------------------------------------------------------ release / cleanup

    def release(
        self, ctx: RequestContext, attempt_id: str, *, remove: bool = False, force: bool = False
    ) -> builtins.list[dict[str, Any]]:
        """Mark an attempt's worktrees released; optionally remove clean ones from disk."""
        with self.db.session(write=True) as session:
            release_leases(session, attempt_id)
            rows = list(
                session.scalars(
                    select(ManagedWorktree).where(
                        ManagedWorktree.attempt_id == attempt_id,
                        ManagedWorktree.status.in_(
                            [WorktreeStatus.ACTIVE, WorktreeStatus.PREPARING, WorktreeStatus.RELEASED]
                        ),
                    )
                )
            )
            for row in rows:
                if row.status != WorktreeStatus.RELEASED:
                    row.status = WorktreeStatus.RELEASED
            targets = [(r.id, r.path, r.source_checkout_id) for r in rows]
        results = []
        if remove:
            for worktree_id, _path, _checkout in targets:
                results.append(self.remove(ctx, worktree_id, force=force))
        return results

    def remove(self, ctx: RequestContext, worktree_id: str, *, force: bool = False) -> dict[str, Any]:
        with self.db.session() as session:
            row = session.get(ManagedWorktree, worktree_id)
            if row is None:
                raise NotFoundError(f"managed worktree {worktree_id} not found")
            from workgraph.persistence.orm import RepositoryCheckout

            checkout = session.get(RepositoryCheckout, row.source_checkout_id)
            source = checkout.path if checkout else None
            path, status, ws_id, attempt_id = Path(row.path), row.status, row.workspace_id, row.attempt_id
        if status == WorktreeStatus.REMOVED:
            return {"worktree_id": worktree_id, "removed": False, "reason": "already removed"}
        if not is_within(path, self.root):
            raise ValidationFailed(f"refusing to remove {path}: not inside Workgraph's worktree directory")
        if path.exists():
            git = Git(path)
            if git.is_repository() and not force and not git.is_clean():
                return {
                    "worktree_id": worktree_id,
                    "removed": False,
                    "reason": "worktree has uncommitted changes",
                }
            if source and Git(source).is_repository():
                args = ["worktree", "remove", str(path)]
                if force:
                    args.insert(2, "--force")
                result = Git(source).run(*args, check=False)
                if result.returncode != 0 and path.exists():
                    return {
                        "worktree_id": worktree_id,
                        "removed": False,
                        "reason": result.stderr.strip()[:300],
                    }
            elif force and path.exists():
                shutil.rmtree(path)
        if source and Git(source).is_repository():
            Git(source).run("worktree", "prune", check=False)
        with self.db.session(write=True) as session:
            row = session.get(ManagedWorktree, worktree_id)
            assert row is not None
            row.status = WorktreeStatus.REMOVED
            row.removed_at = utcnow()
            record_event(
                session,
                ctx,
                event_type="worktree.removed",
                entity_type="worktree",
                entity_id=worktree_id,
                workspace_id=ws_id,
                attempt_id=attempt_id,
                data={"path": str(path), "branch_kept": row.branch},
            )
        parent = path.parent
        if parent.exists() and is_within(parent, self.root) and parent != self.root:
            leftovers = [p for p in parent.iterdir() if p.name != "workgraph-manifest.json"]
            if not leftovers:
                shutil.rmtree(parent, ignore_errors=True)
        return {"worktree_id": worktree_id, "removed": True}

    def recover(self, ctx: RequestContext) -> dict[str, Any]:
        """Crash recovery: reconcile ``managed_worktrees`` rows with disk and attempt state."""
        fixed: dict[str, list[str]] = {"activated": [], "released": [], "orphaned": [], "unknown_dirs": []}
        with self.db.session() as session:
            rows = [
                (r.id, r.path, r.status, r.attempt_id)
                for r in session.scalars(
                    select(ManagedWorktree).where(
                        ManagedWorktree.status.in_([WorktreeStatus.PREPARING, WorktreeStatus.ACTIVE])
                    )
                )
            ]
            attempt_status = {
                a.id: a.status
                for a in session.scalars(
                    select(AgentAttempt).where(AgentAttempt.id.in_([r[3] for r in rows if r[3]]))
                )
            }
            known_paths = {Path(p).resolve() for p in session.scalars(select(ManagedWorktree.path))}
        updates: dict[str, str] = {}
        for worktree_id, path, status, attempt_id in rows:
            exists = Path(path).exists() and Git(path).is_repository()
            finished = attempt_status.get(attempt_id or "") in TERMINAL_ATTEMPT_STATUSES or attempt_id is None
            if not exists:
                updates[worktree_id] = WorktreeStatus.ORPHANED
                fixed["orphaned"].append(path)
            elif finished:
                updates[worktree_id] = WorktreeStatus.RELEASED
                fixed["released"].append(path)
            elif status == WorktreeStatus.PREPARING:
                updates[worktree_id] = WorktreeStatus.ACTIVE
                fixed["activated"].append(path)
        if self.root.exists():
            for attempt_dir in self.root.iterdir():
                if attempt_dir.is_dir():
                    for child in attempt_dir.iterdir():
                        if child.is_dir() and child.resolve() not in known_paths:
                            fixed["unknown_dirs"].append(str(child))
        if updates:
            with self.db.session(write=True) as session:
                for worktree_id, status in updates.items():
                    row = session.get(ManagedWorktree, worktree_id)
                    if row is not None:
                        row.status = status
                        if status == WorktreeStatus.ORPHANED:
                            row.last_error = "worktree missing on disk during recovery"
                    session.flush()
                record_event(
                    session,
                    ctx,
                    event_type="worktree.recovered",
                    entity_type="system",
                    entity_id="worktrees",
                    workspace_id=None,
                    data={k: len(v) for k, v in fixed.items()},
                )
        return fixed

    def list(
        self, ctx: RequestContext, workspace: str | None = None, *, status: str | None = None
    ) -> builtins.list[dict[str, Any]]:
        from workgraph.security.permissions import Permission, require

        require(ctx, Permission.REPO_READ)
        with self.db.session() as session:
            stmt = select(ManagedWorktree, Repository.name).join(
                Repository, Repository.id == ManagedWorktree.repository_id
            )
            if workspace:
                ws_id = self.app.tasks._workspace_id(session, workspace)
                stmt = stmt.where(ManagedWorktree.workspace_id == ws_id)
            if status:
                stmt = stmt.where(ManagedWorktree.status == status)
            return [
                {
                    "id": w.id,
                    "repository": name,
                    "repository_id": w.repository_id,
                    "attempt_id": w.attempt_id,
                    "path": w.path,
                    "branch": w.branch,
                    "base_sha": w.base_sha,
                    "status": w.status,
                    "last_error": w.last_error,
                    "created_at": w.created_at.isoformat(),
                    "removed_at": w.removed_at.isoformat() if w.removed_at else None,
                }
                for w, name in session.execute(stmt.order_by(ManagedWorktree.created_at.desc()))
            ]

    def cleanup(self, ctx: RequestContext, *, dry_run: bool = True) -> builtins.list[dict[str, Any]]:
        """Remove released, clean managed worktrees (explicit operator action, dry-run by default)."""
        from workgraph.security.permissions import Permission, require

        require(ctx, Permission.REPO_ADMIN)
        candidates = self.list(ctx, status=WorktreeStatus.RELEASED)
        if dry_run:
            return [{**c, "action": "would remove (if clean)"} for c in candidates]
        return [self.remove(ctx, c["id"]) for c in candidates]

    def attempt_is_active(self, attempt_id: str) -> bool:
        with self.db.session() as session:
            attempt = session.get(AgentAttempt, attempt_id)
            return attempt is not None and AttemptStatus(attempt.status) not in TERMINAL_ATTEMPT_STATUSES
