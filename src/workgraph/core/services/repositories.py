"""Repository registry.

A repository's canonical identity is its Workgraph UUID. To recognise the *same*
repository at a new path, under a new remote URL, or as an additional clone, we
compare the sorted set of root-commit SHAs (``identity_fingerprint``) and the
normalised remote URLs. Removing a registration never touches files on disk.
"""

from __future__ import annotations

import builtins
from pathlib import Path

from sqlalchemy import delete, select
from sqlalchemy.orm import Session

from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import ConflictError, GitError, NotFoundError, ValidationFailed
from workgraph.core.events.log import record_event
from workgraph.core.fsutil import is_within
from workgraph.core.models.schemas import (
    RepositoryOut,
    RepositoryRegister,
    RepositoryStatus,
    RepositoryUpdate,
)
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.integrations.git.cli import Git, WorktreeEntry, normalize_remote_url
from workgraph.jobs import queue as jobqueue
from workgraph.persistence.orm import (
    ProjectRepository,
    Repository,
    RepositoryCheckout,
    RepositoryDependency,
    RepositoryRemote,
    Workspace,
)
from workgraph.security.permissions import Permission, require


def repository_out(session: Session, repo: Repository) -> RepositoryOut:
    out = RepositoryOut.model_validate(
        {
            **{
                c.key: getattr(repo, c.key)
                for c in Repository.__mapper__.column_attrs
                if c.key != "metadata_"
            },
            "remotes": list(repo.remotes),
            "checkouts": list(repo.checkouts),
        }
    )
    out.project_ids = sorted(
        session.scalars(
            select(ProjectRepository.project_id).where(ProjectRepository.repository_id == repo.id)
        )
    )
    out.depends_on = sorted(
        session.scalars(
            select(RepositoryDependency.to_repository_id).where(
                RepositoryDependency.from_repository_id == repo.id
            )
        )
    )
    return out


def primary_checkout(repo: Repository, *, require_available: bool = True) -> RepositoryCheckout | None:
    candidates = [c for c in repo.checkouts if c.kind != "managed_worktree"]
    candidates.sort(key=lambda c: (not c.is_primary, c.created_at))
    for checkout in candidates:
        if checkout.available or not require_available:
            return checkout
    return None


class RepositoryService(Service):
    def _check_boundary(self, ws: Workspace, path: Path) -> None:
        roots = (ws.settings or {}).get("allowed_roots") or []
        if roots and not any(is_within(path, Path(root)) for root in roots):
            raise ValidationFailed(
                f"{path} is outside the workspace's allowed roots",
                details={"path": str(path), "allowed_roots": roots},
            )
        managed = (self.app.paths.worktrees_dir, self.app.paths.runs_dir, self.app.paths.artifacts_dir)
        if any(is_within(path, root) for root in managed):
            raise ValidationFailed("Workgraph-managed directories cannot be registered as repositories")

    def register(self, ctx: RequestContext, workspace: str | None, data: RepositoryRegister) -> RepositoryOut:
        require(ctx, Permission.REPO_REGISTER)
        raw = Path(data.path).expanduser()
        if not raw.exists() or not raw.is_dir():
            raise ValidationFailed(f"path {raw} does not exist or is not a directory")
        git = Git(raw)
        if not git.is_repository():
            raise ValidationFailed(f"{raw} is not a Git repository")
        top = git.toplevel()
        git = Git(top)
        fingerprint = ",".join(git.root_commits()) or None
        remotes = git.remotes()
        normalized = {name: normalize_remote_url(url) for name, url in remotes.items()}
        kind = "worktree" if git.is_worktree() else "clone"
        default_branch = git.default_branch()
        shallow = git.is_shallow()
        entries = git.worktrees()

        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            ws = session.get(Workspace, ws_id)
            assert ws is not None
            self._check_boundary(ws, top)
            path_str = str(top)
            existing_checkout = session.scalar(
                select(RepositoryCheckout)
                .join(Repository, Repository.id == RepositoryCheckout.repository_id)
                .where(Repository.workspace_id == ws_id, RepositoryCheckout.path == path_str)
            )
            repo: Repository | None = None
            matched_by = None
            if existing_checkout is not None:
                repo = session.get(Repository, existing_checkout.repository_id)
                matched_by = "path"
            if repo is None and fingerprint:
                repo = session.scalar(
                    select(Repository).where(
                        Repository.workspace_id == ws_id, Repository.identity_fingerprint == fingerprint
                    )
                )
                matched_by = "fingerprint" if repo else None
            if repo is None and normalized:
                repo = session.scalar(
                    select(Repository)
                    .join(RepositoryRemote, RepositoryRemote.repository_id == Repository.id)
                    .where(
                        Repository.workspace_id == ws_id,
                        RepositoryRemote.normalized_url.in_(list(normalized.values())),
                    )
                )
                matched_by = "remote" if repo else None

            created = repo is None
            if repo is None:
                name = data.name or top.name
                if session.scalar(
                    select(Repository).where(Repository.workspace_id == ws_id, Repository.name == name)
                ):
                    if data.name:
                        raise ConflictError(f"repository name {name!r} is already registered")
                    suffix = 2
                    while session.scalar(
                        select(Repository).where(
                            Repository.workspace_id == ws_id, Repository.name == f"{name}-{suffix}"
                        )
                    ):
                        suffix += 1
                    name = f"{name}-{suffix}"
                repo = Repository(
                    workspace_id=ws_id,
                    name=name,
                    description=data.description,
                    identity_fingerprint=fingerprint,
                    default_branch=default_branch,
                    tracked_branches=data.tracked_branches or ([default_branch] if default_branch else []),
                    access_mode=data.access_mode,
                    is_shallow=shallow,
                    health_status="ok",
                    health_checked_at=utcnow(),
                )
                session.add(repo)
                session.flush()
            else:
                if repo.archived_at is not None:
                    repo.archived_at = None
                if repo.identity_fingerprint is None and fingerprint:
                    repo.identity_fingerprint = fingerprint
                if data.tracked_branches:
                    repo.tracked_branches = data.tracked_branches

            if existing_checkout is None:
                checkout = RepositoryCheckout(
                    repository_id=repo.id,
                    path=path_str,
                    kind=kind,
                    is_primary=not any(c.kind != "managed_worktree" for c in repo.checkouts),
                    available=True,
                    last_seen_at=utcnow(),
                )
                session.add(checkout)
                repo.checkouts.append(checkout)
            else:
                existing_checkout.available = True
                existing_checkout.last_seen_at = utcnow()
            skipped = self._discover_worktrees(session, ctx, ws, repo, entries, source="register")
            repo.health_detail = {**(repo.health_detail or {}), "skipped_worktrees": skipped}
            self._sync_remotes(repo, remotes, normalized)
            for ref in data.projects:
                project = resolve.project(session, ws_id, ref)
                if session.get(ProjectRepository, (project.id, repo.id)) is None:
                    session.add(ProjectRepository(project_id=project.id, repository_id=repo.id))
            session.flush()
            record_event(
                session,
                ctx,
                event_type="repository.registered" if created else "repository.checkout_added",
                entity_type="repository",
                entity_id=repo.id,
                workspace_id=ws_id,
                data={
                    "name": repo.name,
                    "path": path_str,
                    "kind": kind,
                    "matched_by": matched_by,
                    "fingerprint": fingerprint,
                    "remotes": remotes,
                },
            )
            jobqueue.enqueue(
                session,
                "git_sync",
                {"repository_id": repo.id, "mode": "incremental"},
                workspace_id=ws_id,
                idempotency_key=f"git_sync:{repo.id}:register:{path_str}",
                resource_keys=[f"repo:{repo.id}"],
                created_by=ctx.actor,
            )
            session.flush()
            return repository_out(session, repo)

    @staticmethod
    def _sync_remotes(repo: Repository, remotes: dict[str, str], normalized: dict[str, str]) -> None:
        by_name = {r.name: r for r in repo.remotes}
        for name, url in remotes.items():
            if name in by_name:
                by_name[name].url = url
                by_name[name].normalized_url = normalized[name]
            else:
                repo.remotes.append(RepositoryRemote(name=name, url=url, normalized_url=normalized[name]))

    def _discover_worktrees(
        self,
        session: Session,
        ctx: RequestContext,
        ws: Workspace,
        repo: Repository,
        entries: builtins.list[WorktreeEntry],
        *,
        source: str,
    ) -> builtins.list[dict[str, str]]:
        """Record the main tree and linked worktrees that are not checkouts yet; return what was skipped."""
        known = {c.path for c in repo.checkouts}
        managed = self.app.paths.worktrees_dir
        skipped: builtins.list[dict[str, str]] = []
        for index, entry in enumerate(entries):
            if entry.bare:
                continue
            path = entry.path.resolve()
            path_str = str(path)
            # Workgraph's own managed worktrees are tracked as managed_worktree rows, never as user checkouts.
            if path_str in known or is_within(path, managed):
                continue
            if entry.prunable:
                skipped.append({"path": path_str, "reason": "worktree is prunable"})
                continue
            try:
                self._check_boundary(ws, path)
            except ValidationFailed as exc:
                skipped.append({"path": path_str, "reason": exc.message})
                continue
            kind = "clone" if index == 0 else "worktree"
            checkout = RepositoryCheckout(
                repository_id=repo.id,
                path=path_str,
                kind=kind,
                is_primary=False,
                available=True,
                last_seen_at=utcnow(),
            )
            session.add(checkout)
            repo.checkouts.append(checkout)
            known.add(path_str)
            record_event(
                session,
                ctx,
                event_type="repository.checkout_discovered",
                entity_type="repository",
                entity_id=repo.id,
                workspace_id=ws.id,
                data={
                    "path": path_str,
                    "kind": kind,
                    "branch": entry.branch,
                    "detached": entry.detached,
                    "locked": entry.locked,
                    "source": source,
                },
            )
        return skipped

    def list(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        include_archived: bool = False,
        project: str | None = None,
    ) -> builtins.list[RepositoryOut]:
        require(ctx, Permission.REPO_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            stmt = select(Repository).where(Repository.workspace_id == ws_id).order_by(Repository.name)
            if not include_archived:
                stmt = stmt.where(Repository.archived_at.is_(None))
            if project:
                proj = resolve.project(session, ws_id, project)
                stmt = stmt.where(
                    Repository.id.in_(
                        select(ProjectRepository.repository_id).where(ProjectRepository.project_id == proj.id)
                    )
                )
            return [repository_out(session, r) for r in session.scalars(stmt)]

    def get(self, ctx: RequestContext, workspace: str | None, ref: str) -> RepositoryOut:
        require(ctx, Permission.REPO_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            return repository_out(session, resolve.repository(session, ws_id, ref, include_archived=True))

    def update(
        self, ctx: RequestContext, workspace: str | None, ref: str, data: RepositoryUpdate
    ) -> RepositoryOut:
        require(ctx, Permission.REPO_ADMIN)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            repo = resolve.repository(session, ws_id, ref)
            changes = {}
            for field, value in data.model_dump(exclude_unset=True).items():
                if value is not None and getattr(repo, field) != value:
                    changes[field] = [getattr(repo, field), value]
                    setattr(repo, field, value)
            if changes:
                record_event(
                    session,
                    ctx,
                    event_type="repository.updated",
                    entity_type="repository",
                    entity_id=repo.id,
                    workspace_id=ws_id,
                    data={"changes": changes},
                )
            session.flush()
            return repository_out(session, repo)

    def refresh(self, ctx: RequestContext, workspace: str | None, ref: str) -> RepositoryOut:
        """Health check: verify every checkout, rediscover worktrees, refresh remotes/default branch/shallow state."""
        require(ctx, Permission.REPO_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            repo = resolve.repository(session, ws_id, ref, include_archived=True)
            checkouts = [(c.id, c.path, c.kind) for c in repo.checkouts]
            fingerprint = repo.identity_fingerprint
        observations: dict[str, dict[str, object]] = {}
        remotes: dict[str, str] = {}
        default_branch = None
        shallow = False
        for checkout_id, path, _kind in checkouts:
            git = Git(path)
            ok = git.is_repository()
            detail: dict[str, object] = {"path": path, "available": ok}
            if ok:
                roots = ",".join(git.root_commits()) or None
                if fingerprint and roots and roots != fingerprint:
                    ok = False
                    detail["available"] = False
                    detail["error"] = "history does not match repository identity"
                elif not remotes:
                    remotes = git.remotes()
                    default_branch = git.default_branch()
                    shallow = git.is_shallow()
            else:
                detail["error"] = "path missing or not a git repository"
            observations[checkout_id] = detail
        live = [path for checkout_id, path, _kind in checkouts if observations[checkout_id]["available"]]
        entries = Git(live[0]).worktrees() if live else []
        prunable = {str(entry.path.resolve()) for entry in entries if entry.prunable}
        for checkout_id, path, kind in checkouts:
            if kind == "worktree" and path in prunable:
                observations[checkout_id].update(available=False, error="worktree is prunable")
        with self.db.session(write=True) as session:
            repo = resolve.repository(session, ws_id, ref, include_archived=True)
            ws = session.get(Workspace, ws_id)
            assert ws is not None
            now = utcnow()
            for checkout in repo.checkouts:
                obs = observations.get(checkout.id)
                if obs is None:
                    continue
                available = bool(obs["available"])
                if checkout.kind == "worktree" and available != checkout.available:
                    record_event(
                        session,
                        ctx,
                        event_type="repository.checkout_discovered"
                        if available
                        else "repository.checkout_lost",
                        entity_type="repository",
                        entity_id=repo.id,
                        workspace_id=ws_id,
                        data={"path": checkout.path, "kind": checkout.kind, "reason": obs.get("error")},
                    )
                checkout.available = available
                if checkout.available:
                    checkout.last_seen_at = now
            skipped = self._discover_worktrees(session, ctx, ws, repo, entries, source="refresh")
            if remotes:
                self._sync_remotes(repo, remotes, {k: normalize_remote_url(v) for k, v in remotes.items()})
            if default_branch and not repo.default_branch:
                repo.default_branch = default_branch
            repo.is_shallow = shallow
            available = [c for c in repo.checkouts if c.available and c.kind != "managed_worktree"]
            total = [c for c in repo.checkouts if c.kind != "managed_worktree"]
            status = (
                "ok"
                if available and len(available) == len(total)
                else ("degraded" if available else "unavailable")
            )
            previous = repo.health_status
            repo.health_status = status
            repo.health_detail = {"checkouts": list(observations.values()), "skipped_worktrees": skipped}
            repo.health_checked_at = now
            if previous != status:
                record_event(
                    session,
                    ctx,
                    event_type="repository.health_changed",
                    entity_type="repository",
                    entity_id=repo.id,
                    workspace_id=ws_id,
                    data={"from": previous, "to": status, "detail": repo.health_detail},
                )
            session.flush()
            return repository_out(session, repo)

    def remove(self, ctx: RequestContext, workspace: str | None, ref: str) -> RepositoryOut:
        """Archive the registration. Git data on disk and historical records are untouched."""
        require(ctx, Permission.REPO_ADMIN)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            repo = resolve.repository(session, ws_id, ref)
            repo.archived_at = utcnow()
            record_event(
                session,
                ctx,
                event_type="repository.removed",
                entity_type="repository",
                entity_id=repo.id,
                workspace_id=ws_id,
                data={"name": repo.name, "note": "registration archived; files on disk untouched"},
            )
            session.flush()
            return repository_out(session, repo)

    def remove_checkout(
        self, ctx: RequestContext, workspace: str | None, ref: str, path: str
    ) -> RepositoryOut:
        require(ctx, Permission.REPO_ADMIN)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            repo = resolve.repository(session, ws_id, ref, include_archived=True)
            resolved = str(Path(path).expanduser().resolve())
            target = next((c for c in repo.checkouts if c.path in (path, resolved)), None)
            if target is None:
                raise NotFoundError(f"checkout {path} is not registered for {repo.name}")
            if target.kind == "managed_worktree":
                raise ValidationFailed("managed worktrees are removed through the worktree manager")
            repo.checkouts.remove(target)
            remaining = [c for c in repo.checkouts if c.kind != "managed_worktree"]
            if target.is_primary and remaining:
                remaining[0].is_primary = True
            record_event(
                session,
                ctx,
                event_type="repository.checkout_removed",
                entity_type="repository",
                entity_id=repo.id,
                workspace_id=ws_id,
                data={"path": target.path},
            )
            session.flush()
            return repository_out(session, repo)

    def add_dependency(
        self, ctx: RequestContext, workspace: str | None, ref: str, depends_on: str, description: str = ""
    ) -> RepositoryOut:
        require(ctx, Permission.REPO_ADMIN)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            repo = resolve.repository(session, ws_id, ref)
            other = resolve.repository(session, ws_id, depends_on)
            if repo.id == other.id:
                raise ValidationFailed("a repository cannot depend on itself")
            exists = session.scalar(
                select(RepositoryDependency).where(
                    RepositoryDependency.from_repository_id == repo.id,
                    RepositoryDependency.to_repository_id == other.id,
                )
            )
            if exists is None:
                session.add(
                    RepositoryDependency(
                        from_repository_id=repo.id, to_repository_id=other.id, description=description
                    )
                )
                record_event(
                    session,
                    ctx,
                    event_type="repository.dependency_declared",
                    entity_type="repository",
                    entity_id=repo.id,
                    workspace_id=ws_id,
                    data={"depends_on": other.id, "depends_on_name": other.name},
                )
            session.flush()
            return repository_out(session, repo)

    def remove_dependency(
        self, ctx: RequestContext, workspace: str | None, ref: str, depends_on: str
    ) -> RepositoryOut:
        require(ctx, Permission.REPO_ADMIN)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            repo = resolve.repository(session, ws_id, ref)
            other = resolve.repository(session, ws_id, depends_on, include_archived=True)
            session.execute(
                delete(RepositoryDependency).where(
                    RepositoryDependency.from_repository_id == repo.id,
                    RepositoryDependency.to_repository_id == other.id,
                )
            )
            session.flush()
            return repository_out(session, repo)

    def status(self, ctx: RequestContext, workspace: str | None, ref: str) -> RepositoryStatus:
        require(ctx, Permission.REPO_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            repo = resolve.repository(session, ws_id, ref, include_archived=True)
            checkout = primary_checkout(repo)
            repo_id, name = repo.id, repo.name
            path = checkout.path if checkout else None
        if path is None:
            return RepositoryStatus(
                repository_id=repo_id,
                name=name,
                checkout_path=None,
                available=False,
                error="no available checkout",
            )
        git = Git(path)
        try:
            info = git.status_porcelain()
        except GitError as exc:
            return RepositoryStatus(
                repository_id=repo_id, name=name, checkout_path=path, available=False, error=exc.message
            )
        return RepositoryStatus(
            repository_id=repo_id,
            name=name,
            checkout_path=path,
            available=True,
            branch=info.branch,
            head=info.head,
            detached=info.branch is None,
            staged=info.staged,
            unstaged=info.unstaged,
            untracked=info.untracked,
            ahead=info.ahead,
            behind=info.behind,
            is_shallow=git.is_shallow(),
        )
