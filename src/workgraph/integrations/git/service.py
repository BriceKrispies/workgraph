"""Git ingestion, reconciliation, attribution and change measurement.

Design rules:

* The Git CLI is the source of truth; Workgraph stores stable metadata (commits,
  parents, trailers, per-file statistics, symbols) but never full diff payloads.
  Diffs are re-read from Git on demand, and missing objects are recorded.
* Git subprocesses never run inside a SQLite write transaction.
* History is never rewritten. Rewritten (rebased/amended) commits are reconciled
  by marking old commits unreachable and recording equivalences; links carry over
  with ``rewrite_reconciliation`` provenance.
* Cumulative churn (sum over implementation commits) and the final change surface
  (baseline versus accepted revision) are separate measurements.
"""

from __future__ import annotations

import builtins
from collections import defaultdict
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any

from pydantic import BaseModel
from sqlalchemy import func, select, update
from sqlalchemy.orm import Session

from workgraph.analytics.code.classify import file_class, language_for, module_for
from workgraph.analytics.code.structure import ANALYZER_VERSION, analyze_file, parse_hunks
from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import GitError, NotFoundError, ValidationFailed
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import ATTRIBUTION_CONFIDENCE, AttributionSource
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.core.services.repositories import primary_checkout
from workgraph.integrations.git.attribution import message_candidates, trailer_candidates
from workgraph.integrations.git.cli import Git
from workgraph.integrations.git.parse import (
    CHERRY_PICK_RE,
    META_FORMAT,
    parse_diff_tree,
    parse_meta,
    parse_numstat_z,
    parse_patch_ids,
)
from workgraph.jobs import queue as jobqueue
from workgraph.persistence.orm import (
    AgentAttempt,
    ChangeSet,
    ChangeSetItem,
    Commit,
    CommitAnalysis,
    CommitEquivalence,
    CommitFileChange,
    CommitSymbol,
    CommitTaskLink,
    GitObjectGap,
    ManagedWorktree,
    Repository,
    RepositoryRef,
    RevisionManifest,
    Task,
    TaskAcceptance,
    TaskChangeSurface,
    Workspace,
)
from workgraph.security.permissions import Permission, require

BATCH = 400
EMPTY_TREE = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"  # Git's well-known empty tree object
PER_COMMIT_EVENT_LIMIT = 200


class SyncResult(BaseModel):
    repository_id: str
    repository: str
    mode: str
    new_commits: int = 0
    refs_changed: int = 0
    marked_unreachable: int = 0
    restored_reachable: int = 0
    equivalences: int = 0
    links_created: int = 0
    shallow: bool = False
    head_refs: int = 0


class CommitLinkOut(BaseModel):
    id: str
    commit_id: str
    sha: str
    repository_id: str
    task_id: str
    source: str
    confidence: float
    evidence: dict[str, Any]
    attempt_id: str | None
    created_at: datetime


@dataclass
class _RepoInfo:
    id: str
    name: str
    workspace_id: str
    key_prefix: str
    path: str
    tracked_branches: list[str] = field(default_factory=list)
    known_refs: dict[str, str] = field(default_factory=dict)


def _link_out(link: CommitTaskLink, commit: Commit) -> CommitLinkOut:
    return CommitLinkOut(
        id=link.id,
        commit_id=commit.id,
        sha=commit.sha,
        repository_id=commit.repository_id,
        task_id=link.task_id,
        source=link.source,
        confidence=link.confidence,
        evidence=link.evidence,
        attempt_id=link.attempt_id,
        created_at=link.created_at,
    )


class GitService(Service):
    # ------------------------------------------------------------------ helpers

    def _repo_info(self, repository_id: str) -> _RepoInfo:
        with self.db.session() as session:
            repo = session.get(Repository, repository_id)
            if repo is None:
                raise NotFoundError(f"repository {repository_id} not found")
            checkout = primary_checkout(repo)
            if checkout is None:
                raise GitError(f"repository {repo.name} has no available checkout")
            ws = session.get(Workspace, repo.workspace_id)
            assert ws is not None
            refs = {
                r.name: r.sha
                for r in session.scalars(select(RepositoryRef).where(RepositoryRef.repository_id == repo.id))
            }
            return _RepoInfo(
                id=repo.id,
                name=repo.name,
                workspace_id=repo.workspace_id,
                key_prefix=ws.key_prefix,
                path=checkout.path,
                tracked_branches=list(repo.tracked_branches or []),
                known_refs=refs,
            )

    # ------------------------------------------------------------------ sync

    def sync_workspace(
        self, ctx: RequestContext, workspace: str | None, mode: str = "incremental"
    ) -> list[SyncResult]:
        require(ctx, Permission.GIT_INGEST)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            ids = list(
                session.scalars(
                    select(Repository.id).where(
                        Repository.workspace_id == ws_id, Repository.archived_at.is_(None)
                    )
                )
            )
        results = []
        for repo_id in ids:
            try:
                results.append(self.sync(ctx, repo_id, mode=mode))
            except GitError as exc:
                results.append(
                    SyncResult(repository_id=repo_id, repository=f"error: {exc.message}", mode=mode)
                )
        return results

    def sync_ref(
        self, ctx: RequestContext, workspace: str | None, repository: str, mode: str = "incremental"
    ) -> SyncResult:
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            repo_id = resolve.repository(session, ws_id, repository).id
        return self.sync(ctx, repo_id, mode=mode)

    def sync(self, ctx: RequestContext, repository_id: str, *, mode: str = "incremental") -> SyncResult:
        """Ingest new commits. ``full`` additionally reconciles reachability of all known commits."""
        require(ctx, Permission.GIT_INGEST)
        if mode not in ("incremental", "full"):
            raise ValidationFailed("mode must be incremental or full")
        info = self._repo_info(repository_id)
        git = Git(info.path, timeout=600)
        refs = git.refs()
        shallow = git.is_shallow()
        result = SyncResult(
            repository_id=info.id, repository=info.name, mode=mode, shallow=shallow, head_refs=len(refs)
        )
        changed_refs = {name for name, sha in refs.items() if info.known_refs.get(name) != sha}
        removed_refs = set(info.known_refs) - set(refs)
        result.refs_changed = len(changed_refs) + len(removed_refs)
        non_fast_forward = bool(removed_refs) or any(
            info.known_refs.get(name) and not git.is_ancestor(info.known_refs[name], refs[name])
            for name in changed_refs
            if name in info.known_refs
        )

        with self.db.session() as session:
            known = set(session.scalars(select(Commit.sha).where(Commit.repository_id == info.id)))
        if mode == "full" or not info.known_refs:
            candidates = git.out("rev-list", "--all").split() if refs else []
        else:
            old_tips = [sha for sha in set(info.known_refs.values()) if git.object_exists(sha)]
            stdin = "\n".join(f"^{sha}" for sha in old_tips)
            candidates = (
                git.run("rev-list", "--all", "--stdin", input_text=stdin + "\n").stdout.split()
                if refs
                else []
            )
        new_shas = [sha for sha in reversed(candidates) if sha not in known]  # oldest first

        new_commit_ids: list[str] = []
        for start in range(0, len(new_shas), BATCH):
            new_commit_ids += self._ingest_batch(git, info, new_shas[start : start + BATCH])
        result.new_commits = len(new_commit_ids)
        if new_commit_ids:
            self._update_branch_membership(git, info, set(new_shas))

        if mode == "full" or non_fast_forward:
            unreachable, restored = self._reconcile_reachability(git, info)
            result.marked_unreachable = unreachable
            result.restored_reachable = restored

        result.equivalences = self._detect_equivalences(info, new_commit_ids)
        result.links_created = self._attribute(ctx, info, new_commit_ids)

        with self.db.session(write=True) as session:
            existing = {
                r.name: r
                for r in session.scalars(select(RepositoryRef).where(RepositoryRef.repository_id == info.id))
            }
            for name, sha in refs.items():
                if name in existing:
                    existing[name].sha = sha
                else:
                    session.add(RepositoryRef(repository_id=info.id, name=name, sha=sha))
            for name in removed_refs:
                if name in existing:
                    session.delete(existing[name])
            repo = session.get(Repository, info.id)
            assert repo is not None
            repo.last_scanned_at = utcnow()
            repo.is_shallow = shallow
            if shallow:
                repo.health_detail = {**(repo.health_detail or {}), "shallow": True}
            summary = record_event(
                session,
                ctx,
                event_type="git.sync_completed",
                entity_type="repository",
                entity_id=info.id,
                workspace_id=info.workspace_id,
                data=result.model_dump(),
            )
            if result.refs_changed:
                record_event(
                    session,
                    ctx.caused_by(summary.id),
                    event_type="repository.changes_detected",
                    entity_type="repository",
                    entity_id=info.id,
                    workspace_id=info.workspace_id,
                    data={
                        "changed_refs": sorted(changed_refs)[:100],
                        "removed_refs": sorted(removed_refs)[:100],
                        "non_fast_forward": non_fast_forward,
                    },
                )
            if new_commit_ids and len(new_commit_ids) <= PER_COMMIT_EVENT_LIMIT and info.known_refs:
                for commit in session.scalars(select(Commit).where(Commit.id.in_(new_commit_ids))):
                    task_ids = list(
                        session.scalars(
                            select(CommitTaskLink.task_id).where(CommitTaskLink.commit_id == commit.id)
                        )
                    )
                    record_event(
                        session,
                        ctx.caused_by(summary.id),
                        event_type="git.commit_ingested",
                        entity_type="commit",
                        entity_id=commit.id,
                        workspace_id=info.workspace_id,
                        data={
                            "repository_id": info.id,
                            "sha": commit.sha,
                            "subject": commit.subject[:200],
                            "task_ids": task_ids,
                            "insertions": commit.insertions,
                            "deletions": commit.deletions,
                        },
                    )
            analysable = list(
                session.scalars(
                    select(Commit.id).where(Commit.id.in_(new_commit_ids), Commit.is_merge.is_(False))
                )
            )
            for start in range(0, len(analysable), 100):
                chunk = analysable[start : start + 100]
                jobqueue.enqueue(
                    session,
                    "analyze_commits",
                    {"commit_ids": chunk},
                    workspace_id=info.workspace_id,
                    priority=200,
                    idempotency_key=f"analyze:{chunk[0]}:{len(chunk)}",
                    resource_keys=[f"repo:{info.id}"],
                    causation_id=summary.id,
                )
        return result

    def _ingest_batch(self, git: Git, info: _RepoInfo, shas: builtins.list[str]) -> builtins.list[str]:
        if not shas:
            return []
        metas = parse_meta(
            git.run(
                "log",
                "--no-walk=unsorted",
                "--stdin",
                f"--format={META_FORMAT}",
                input_text="\n".join(shas) + "\n",
            ).stdout
        )
        by_sha = {m.sha: m for m in metas}
        lines = []
        for sha in shas:
            meta = by_sha.get(sha)
            if meta is None:
                continue
            lines.append(f"{sha} {meta.parents[0]}" if meta.parents else sha)
        diffs = parse_diff_tree(
            git.run(
                "diff-tree",
                "--stdin",
                "-z",
                "-r",
                "-M",
                "-C",
                "--root",
                "--raw",
                "--numstat",
                input_text="\n".join(lines) + "\n",
            ).stdout
        )
        single_parent = [
            f"{meta.sha} {meta.parents[0]}" if meta.parents else meta.sha
            for meta in (by_sha[sha] for sha in shas if sha in by_sha)
            if len(meta.parents) <= 1
        ]
        patch_ids: dict[str, str] = {}
        if single_parent:
            patches = git.run(
                "diff-tree", "--stdin", "-p", "-r", "--root", input_text="\n".join(single_parent) + "\n"
            ).stdout
            if patches.strip():
                patch_ids = parse_patch_ids(git.run("patch-id", "--stable", input_text=patches).stdout)
        created: list[str] = []
        with self.db.session(write=True) as session:
            for sha in shas:
                meta = by_sha.get(sha)
                if meta is None:
                    continue
                diff = diffs.get(sha)
                files = diff.files if diff else []
                commit = Commit(
                    repository_id=info.id,
                    sha=sha,
                    tree_sha=meta.tree,
                    parents=meta.parents,
                    author_name=meta.author_name,
                    author_email=meta.author_email,
                    authored_at=meta.authored_at,
                    committer_name=meta.committer_name,
                    committer_email=meta.committer_email,
                    committed_at=meta.committed_at,
                    subject=meta.subject[:1000],
                    message=meta.message[:100_000],
                    trailers=meta.trailers,
                    is_merge=len(meta.parents) > 1,
                    patch_id=patch_ids.get(sha),
                    insertions=sum(f.insertions for f in files),
                    deletions=sum(f.deletions for f in files),
                    files_changed=len(files),
                )
                session.add(commit)
                session.flush()
                for f in files:
                    session.add(
                        CommitFileChange(
                            commit_id=commit.id,
                            path=f.path,
                            old_path=f.old_path,
                            change_type=f.status,
                            similarity=f.similarity,
                            insertions=f.insertions,
                            deletions=f.deletions,
                            is_binary=f.binary,
                            language=language_for(f.path),
                            file_class=file_class(f.path),
                            module=module_for(f.path),
                        )
                    )
                created.append(commit.id)
        return created

    def _update_branch_membership(self, git: Git, info: _RepoInfo, new_shas: set[str]) -> None:
        branches = list(dict.fromkeys([*info.tracked_branches, *git.local_branches()]))[:20]
        membership: dict[str, builtins.list[str]] = defaultdict(list)
        for branch in branches:
            result = git.run("rev-list", f"refs/heads/{branch}", check=False)
            if result.returncode != 0:
                continue
            for sha in result.stdout.split():
                if sha in new_shas:
                    membership[sha].append(branch)
        if not membership:
            return
        with self.db.session(write=True) as session:
            for commit in session.scalars(
                select(Commit).where(Commit.repository_id == info.id, Commit.sha.in_(list(membership)))
            ):
                commit.branches = sorted(set(commit.branches or []) | set(membership[commit.sha]))

    def _reconcile_reachability(self, git: Git, info: _RepoInfo) -> tuple[int, int]:
        reachable = set(git.out("rev-list", "--all").split())
        with self.db.session() as session:
            rows = session.execute(
                select(Commit.id, Commit.sha, Commit.reachable).where(Commit.repository_id == info.id)
            ).all()
        to_unreach = [(cid, sha) for cid, sha, r in rows if r and sha not in reachable]
        to_restore = [cid for cid, sha, r in rows if not r and sha in reachable]
        availability = {cid: git.object_exists(sha) for cid, sha in to_unreach}
        now = utcnow()
        with self.db.session(write=True) as session:
            for cid, _sha in to_unreach:
                session.execute(
                    update(Commit)
                    .where(Commit.id == cid)
                    .values(reachable=False, unreachable_since=now, object_available=availability[cid])
                )
            if to_restore:
                session.execute(
                    update(Commit)
                    .where(Commit.id.in_(to_restore))
                    .values(reachable=True, unreachable_since=None, object_available=True)
                )
        return len(to_unreach), len(to_restore)

    def _detect_equivalences(self, info: _RepoInfo, new_commit_ids: builtins.list[str]) -> int:
        """Pair new commits with older commits carrying the same change.

        * same patch-id, older commit unreachable  -> ``rewrite`` (rebase)
        * same patch-id, older commit reachable    -> ``cherry_pick``
        * same author, author-date and subject, older unreachable -> ``rewrite`` (amend)
        * "(cherry picked from commit X)" message  -> ``cherry_pick``
        """
        if not new_commit_ids:
            return 0
        created = 0
        with self.db.session(write=True) as session:
            new_set = set(new_commit_ids)
            for commit in list(session.scalars(select(Commit).where(Commit.id.in_(new_commit_ids)))):
                matches: list[tuple[Commit, str, dict[str, Any]]] = []
                if commit.patch_id:
                    for other in session.scalars(
                        select(Commit).where(
                            Commit.repository_id == info.id,
                            Commit.patch_id == commit.patch_id,
                            Commit.id != commit.id,
                        )
                    ):
                        if other.id in new_set and other.reachable is not False:
                            continue
                        kind = "rewrite" if not other.reachable else "cherry_pick"
                        matches.append((other, kind, {"method": "patch-id", "patch_id": commit.patch_id}))
                if not matches and commit.authored_at is not None:
                    for other in session.scalars(
                        select(Commit).where(
                            Commit.repository_id == info.id,
                            Commit.id != commit.id,
                            Commit.reachable.is_(False),
                            Commit.author_email == commit.author_email,
                            Commit.authored_at == commit.authored_at,
                            Commit.subject == commit.subject,
                        )
                    ):
                        matches.append((other, "rewrite", {"method": "author-date+subject"}))
                picked = None
                meta_cherry = None
                if "(cherry picked from commit" in commit.message:
                    found = CHERRY_PICK_RE.search(commit.message)
                    meta_cherry = found.group(1) if found else None
                if meta_cherry:
                    picked = session.scalar(
                        select(Commit).where(
                            Commit.repository_id == info.id, Commit.sha.startswith(meta_cherry)
                        )
                    )
                    if picked is not None and all(m[0].id != picked.id for m in matches):
                        matches.append((picked, "cherry_pick", {"method": "cherry-pick trailer"}))
                for other, kind, evidence in matches:
                    exists = session.scalar(
                        select(CommitEquivalence).where(
                            CommitEquivalence.original_commit_id == other.id,
                            CommitEquivalence.equivalent_commit_id == commit.id,
                        )
                    )
                    if exists is not None:
                        continue
                    session.add(
                        CommitEquivalence(
                            repository_id=info.id,
                            original_commit_id=other.id,
                            equivalent_commit_id=commit.id,
                            kind=kind,
                            evidence={**evidence, "original_sha": other.sha, "new_sha": commit.sha},
                        )
                    )
                    created += 1
                    if kind == "rewrite":
                        self._carry_links(session, other, commit)
        return created

    @staticmethod
    def _carry_links(session: Session, original: Commit, rewritten: Commit) -> None:
        for link in session.scalars(select(CommitTaskLink).where(CommitTaskLink.commit_id == original.id)):
            exists = session.scalar(
                select(CommitTaskLink).where(
                    CommitTaskLink.commit_id == rewritten.id,
                    CommitTaskLink.task_id == link.task_id,
                    CommitTaskLink.source == AttributionSource.REWRITE_RECONCILIATION,
                )
            )
            if exists is None:
                session.add(
                    CommitTaskLink(
                        commit_id=rewritten.id,
                        task_id=link.task_id,
                        source=AttributionSource.REWRITE_RECONCILIATION,
                        confidence=min(
                            link.confidence, ATTRIBUTION_CONFIDENCE[AttributionSource.REWRITE_RECONCILIATION]
                        ),
                        evidence={"from_commit": original.sha, "original_source": link.source},
                        attempt_id=link.attempt_id,
                        created_by="system:git-reconciliation",
                    )
                )

    # ------------------------------------------------------------------ attribution

    def _attribute(self, ctx: RequestContext, info: _RepoInfo, commit_ids: builtins.list[str]) -> int:
        if not commit_ids:
            return 0
        created = 0
        worktree_branches = self._managed_branch_commits(info)
        with self.db.session(write=True) as session:
            for commit in list(session.scalars(select(Commit).where(Commit.id.in_(commit_ids)))):
                candidates = trailer_candidates(commit.trailers or [])
                candidates += message_candidates(commit.message, info.key_prefix, commit.trailers or [])
                for candidate in candidates:
                    try:
                        task = resolve.task(session, info.workspace_id, candidate.task_ref)
                    except (NotFoundError, ValidationFailed):
                        continue
                    created += self._add_link(
                        session,
                        commit,
                        task.id,
                        candidate.source,
                        candidate.confidence,
                        candidate.evidence,
                        None,
                        "system:git-attribution",
                    )
                wt = worktree_branches.get(commit.sha)
                if wt is not None:
                    attempt_id, task_id, branch = wt
                    created += self._add_link(
                        session,
                        commit,
                        task_id,
                        AttributionSource.WORKTREE,
                        ATTRIBUTION_CONFIDENCE[AttributionSource.WORKTREE],
                        {"worktree_branch": branch},
                        attempt_id,
                        "system:git-attribution",
                    )
            if created:
                record_event(
                    session,
                    ctx,
                    event_type="git.commits_attributed",
                    entity_type="repository",
                    entity_id=info.id,
                    workspace_id=info.workspace_id,
                    data={"links_created": created},
                )
        return created

    def _managed_branch_commits(self, info: _RepoInfo) -> dict[str, tuple[str | None, str, str]]:
        with self.db.session() as session:
            rows = session.execute(
                select(
                    ManagedWorktree.branch,
                    ManagedWorktree.base_sha,
                    ManagedWorktree.attempt_id,
                    AgentAttempt.task_id,
                )
                .join(AgentAttempt, AgentAttempt.id == ManagedWorktree.attempt_id)
                .where(ManagedWorktree.repository_id == info.id)
            ).all()
        if not rows:
            return {}
        git = Git(info.path)
        mapping: dict[str, tuple[str | None, str, str]] = {}
        for branch, base, attempt_id, task_id in rows:
            result = git.run("rev-list", f"{base}..refs/heads/{branch}", check=False)
            if result.returncode == 0:
                for sha in result.stdout.split():
                    mapping.setdefault(sha, (attempt_id, task_id, branch))
        return mapping

    @staticmethod
    def _add_link(
        session: Session,
        commit: Commit,
        task_id: str,
        source: str,
        confidence: float,
        evidence: dict[str, Any],
        attempt_id: str | None,
        actor: str,
    ) -> int:
        exists = session.scalar(
            select(CommitTaskLink).where(
                CommitTaskLink.commit_id == commit.id,
                CommitTaskLink.task_id == task_id,
                CommitTaskLink.source == source,
            )
        )
        if exists is not None:
            return 0
        session.add(
            CommitTaskLink(
                commit_id=commit.id,
                task_id=task_id,
                source=source,
                confidence=confidence,
                evidence=evidence,
                attempt_id=attempt_id,
                created_by=actor,
            )
        )
        session.flush()
        return 1

    def ensure_commit(self, repository_id: str, sha: str) -> str:
        """Return the commit id, ingesting the commit on demand if Git has it."""
        info = self._repo_info(repository_id)
        git = Git(info.path)
        try:
            full = git.resolve_commit(sha)
        except GitError as exc:
            raise NotFoundError(f"commit {sha} not found in {info.name}") from exc
        with self.db.session() as session:
            existing = session.scalar(
                select(Commit.id).where(Commit.repository_id == repository_id, Commit.sha == full)
            )
        if existing:
            return existing
        created = self._ingest_batch(git, info, [full])
        if not created:
            raise NotFoundError(f"commit {sha} could not be ingested from {info.name}")
        self.analyze_commits(created)
        return created[0]

    def link_commit(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        sha: str,
        *,
        repository: str | None = None,
        note: str = "",
    ) -> builtins.list[CommitLinkOut]:
        """Explicitly associate a commit with a task (confidence 1.0)."""
        require(ctx, Permission.GIT_LINK)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            task = resolve.task(session, ws_id, task_ref)
            task_id = task.id
            if repository:
                repo_ids = [resolve.repository(session, ws_id, repository).id]
            else:
                from workgraph.persistence.orm import TaskRepository

                repo_ids = list(
                    session.scalars(
                        select(TaskRepository.repository_id).where(TaskRepository.task_id == task_id)
                    )
                )
                if not repo_ids:
                    repo_ids = list(
                        session.scalars(
                            select(Repository.id).where(
                                Repository.workspace_id == ws_id, Repository.archived_at.is_(None)
                            )
                        )
                    )
        found: tuple[str, str] | None = None
        for repo_id in repo_ids:
            try:
                found = (repo_id, self.ensure_commit(repo_id, sha))
                break
            except (NotFoundError, GitError):
                continue
        if found is None:
            raise NotFoundError(f"commit {sha} not found in the task's repositories")
        repo_id, commit_id = found
        with self.db.session(write=True) as session:
            commit = session.get(Commit, commit_id)
            assert commit is not None
            self._add_link(
                session,
                commit,
                task_id,
                AttributionSource.EXPLICIT,
                1.0,
                {"note": note, "linked_by": ctx.actor},
                ctx.attempt_id,
                ctx.actor,
            )
            from workgraph.persistence.orm import TaskRepository

            if session.get(TaskRepository, (task_id, repo_id)) is None:
                repo = session.get(Repository, repo_id)
                session.add(
                    TaskRepository(
                        task_id=task_id,
                        repository_id=repo_id,
                        access=repo.access_mode if repo else "read_write",
                    )
                )
            record_event(
                session,
                ctx,
                event_type="task.commit_linked",
                entity_type="task",
                entity_id=task_id,
                workspace_id=ws_id,
                data={
                    "commit_id": commit_id,
                    "sha": commit.sha,
                    "repository_id": repo_id,
                    "source": "explicit",
                },
            )
            links = session.scalars(
                select(CommitTaskLink).where(
                    CommitTaskLink.commit_id == commit_id, CommitTaskLink.task_id == task_id
                )
            )
            return [_link_out(link, commit) for link in links]

    def unlink_commit(self, ctx: RequestContext, workspace: str | None, task_ref: str, sha: str) -> int:
        """Remove *explicit* links only; inferred provenance stays as evidence."""
        require(ctx, Permission.GIT_LINK)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            task = resolve.task(session, ws_id, task_ref)
            links = list(
                session.scalars(
                    select(CommitTaskLink)
                    .join(Commit, Commit.id == CommitTaskLink.commit_id)
                    .where(
                        CommitTaskLink.task_id == task.id,
                        Commit.sha.startswith(sha),
                        CommitTaskLink.source == AttributionSource.EXPLICIT,
                    )
                )
            )
            for link in links:
                session.delete(link)
            if links:
                record_event(
                    session,
                    ctx,
                    event_type="task.commit_unlinked",
                    entity_type="task",
                    entity_id=task.id,
                    workspace_id=ws_id,
                    data={"sha": sha, "removed": len(links)},
                )
            return len(links)

    def link_attempt_commits(self, ctx: RequestContext, attempt_id: str) -> int:
        """Attribute commits between an attempt's baseline and outcome manifests (execution context)."""
        with self.db.session() as session:
            attempt = resolve.attempt(session, attempt_id)
            baseline = (
                session.get(RevisionManifest, attempt.baseline_manifest_id)
                if attempt.baseline_manifest_id
                else None
            )
            outcome = (
                session.get(RevisionManifest, attempt.outcome_manifest_id)
                if attempt.outcome_manifest_id
                else None
            )
            if baseline is None or outcome is None:
                return 0
            base_by_repo = {e["repository_id"]: e for e in baseline.entries}
            pairs = [
                (
                    e["repository_id"],
                    base_by_repo[e["repository_id"]]["sha"],
                    e["sha"],
                    e.get("checkout_path"),
                )
                for e in outcome.entries
                if e["repository_id"] in base_by_repo
                and e.get("sha")
                and base_by_repo[e["repository_id"]].get("sha")
            ]
            task_id, ws_id = attempt.task_id, attempt.workspace_id
        created = 0
        for repo_id, base, head, path in pairs:
            if base == head or not path:
                continue
            git = Git(path)
            shas = git.out("rev-list", "--reverse", f"{base}..{head}").split()
            info = self._repo_info(repo_id)
            with self.db.session() as session:
                known = set(
                    session.scalars(
                        select(Commit.sha).where(Commit.repository_id == repo_id, Commit.sha.in_(shas))
                    )
                )
            missing = [s for s in shas if s not in known]
            if missing:
                new_ids = self._ingest_batch(git, info, missing)
                self._detect_equivalences(info, new_ids)
                # Attempt commits are few and needed for churn metrics right away.
                self.analyze_commits(new_ids)
            with self.db.session(write=True) as session:
                for commit in session.scalars(
                    select(Commit).where(Commit.repository_id == repo_id, Commit.sha.in_(shas))
                ):
                    created += self._add_link(
                        session,
                        commit,
                        task_id,
                        AttributionSource.EXECUTION_CONTEXT,
                        ATTRIBUTION_CONFIDENCE[AttributionSource.EXECUTION_CONTEXT],
                        {"attempt_id": attempt_id, "range": f"{base[:12]}..{head[:12]}"},
                        attempt_id,
                        "system:runner",
                    )
                if created:
                    record_event(
                        session,
                        ctx,
                        event_type="git.commits_attributed",
                        entity_type="attempt",
                        entity_id=attempt_id,
                        workspace_id=ws_id,
                        attempt_id=attempt_id,
                        data={"links_created": created, "repository_id": repo_id},
                    )
        return created

    # ------------------------------------------------------------------ queries

    def task_commits(
        self, ctx: RequestContext, workspace: str | None, task_ref: str, *, min_confidence: float = 0.0
    ) -> builtins.list[dict[str, Any]]:
        require(ctx, Permission.GIT_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            return self._task_commits(session, task.id, min_confidence)

    def _task_commits(
        self, session: Session, task_id: str, min_confidence: float = 0.0
    ) -> builtins.list[dict[str, Any]]:
        rows = session.execute(
            select(CommitTaskLink, Commit, Repository.name)
            .join(Commit, Commit.id == CommitTaskLink.commit_id)
            .join(Repository, Repository.id == Commit.repository_id)
            .where(CommitTaskLink.task_id == task_id, CommitTaskLink.confidence >= min_confidence)
            .order_by(Commit.committed_at)
        ).all()
        by_commit: dict[str, dict[str, Any]] = {}
        for link, commit, repo_name in rows:
            entry = by_commit.setdefault(
                commit.id,
                {
                    "commit_id": commit.id,
                    "sha": commit.sha,
                    "repository_id": commit.repository_id,
                    "repository": repo_name,
                    "subject": commit.subject,
                    "author": commit.author_name,
                    "committed_at": commit.committed_at.isoformat() if commit.committed_at else None,
                    "insertions": commit.insertions,
                    "deletions": commit.deletions,
                    "files_changed": commit.files_changed,
                    "is_merge": commit.is_merge,
                    "reachable": commit.reachable,
                    "object_available": commit.object_available,
                    "links": [],
                    "confidence": 0.0,
                },
            )
            entry["links"].append(
                {
                    "source": link.source,
                    "confidence": link.confidence,
                    "evidence": link.evidence,
                    "attempt_id": link.attempt_id,
                }
            )
            entry["confidence"] = max(entry["confidence"], link.confidence)
        return list(by_commit.values())

    def task_changes(
        self, ctx: RequestContext, workspace: str | None, task_ref: str, *, min_confidence: float = 0.5
    ) -> dict[str, Any]:
        """Cumulative churn over linked implementation commits, plus the final change surface."""
        require(ctx, Permission.GIT_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            commits = self._task_commits(session, task.id, min_confidence)
            churn = self._cumulative_churn(session, [c["commit_id"] for c in commits])
            surfaces = [
                {
                    "repository_id": s.repository_id,
                    "base_sha": s.base_sha,
                    "final_sha": s.final_sha,
                    "files_changed": s.files_changed,
                    "insertions": s.insertions,
                    "deletions": s.deletions,
                    "renames": s.renames,
                    "binary_files": s.binary_files,
                    "functions_affected": s.functions_affected,
                    "types_affected": s.types_affected,
                    "detail": s.detail,
                }
                for s in session.scalars(
                    select(TaskChangeSurface).where(TaskChangeSurface.task_id == task.id)
                )
            ]
            return {
                "task_id": task.id,
                "task_key": task.key,
                "min_confidence": min_confidence,
                "commits": commits,
                "cumulative_churn": churn,
                "final_change_surface": surfaces,
                "definitions": {
                    "cumulative_churn": "Sum of lines added/removed across distinct linked non-merge commits; "
                    "cherry-picked/rewritten duplicates of the same change are counted once.",
                    "final_change_surface": "git diff between the attempt baseline and the accepted revision, per repository.",
                },
            }

    def _cumulative_churn(self, session: Session, commit_ids: builtins.list[str]) -> dict[str, Any]:
        if not commit_ids:
            return {
                "commits": 0,
                "insertions": 0,
                "deletions": 0,
                "files_touched": 0,
                "repeated_file_edits": 0,
                "functions_affected": 0,
                "types_affected": 0,
                "excluded_duplicates": 0,
                "excluded_merges": 0,
            }
        commits = list(session.scalars(select(Commit).where(Commit.id.in_(commit_ids))))
        # Keyed by (repository, patch id): identical patches in different repositories are distinct work.
        seen_patches: set[tuple[str, str]] = set()
        counted: list[Commit] = []
        duplicates = merges = 0
        for commit in sorted(commits, key=lambda c: c.committed_at or utcnow()):
            if commit.is_merge:
                merges += 1
                continue
            if commit.patch_id and (commit.repository_id, commit.patch_id) in seen_patches:
                duplicates += 1
                continue
            if commit.patch_id:
                seen_patches.add((commit.repository_id, commit.patch_id))
            counted.append(commit)
        counted_ids = [c.id for c in counted]
        file_rows = session.execute(
            select(
                CommitFileChange.path,
                func.count(),
                func.sum(CommitFileChange.insertions),
                func.sum(CommitFileChange.deletions),
            )
            .where(CommitFileChange.commit_id.in_(counted_ids))
            .group_by(CommitFileChange.path)
        ).all()
        analyses = list(
            session.scalars(select(CommitAnalysis).where(CommitAnalysis.commit_id.in_(counted_ids)))
        )
        by_class: dict[str, dict[str, int]] = defaultdict(
            lambda: {"insertions": 0, "deletions": 0, "files": 0}
        )
        for fc in session.scalars(
            select(CommitFileChange).where(CommitFileChange.commit_id.in_(counted_ids))
        ):
            bucket = by_class[fc.file_class]
            bucket["insertions"] += fc.insertions
            bucket["deletions"] += fc.deletions
            bucket["files"] += 1
        return {
            "commits": len(counted),
            "insertions": sum(c.insertions for c in counted),
            "deletions": sum(c.deletions for c in counted),
            "files_touched": len(file_rows),
            "repeated_file_edits": sum(1 for row in file_rows if row[1] > 1),
            "functions_affected": sum(a.functions_affected for a in analyses),
            "types_affected": sum(a.types_affected for a in analyses),
            "structurally_analyzed_commits": len(analyses),
            "excluded_duplicates": duplicates,
            "excluded_merges": merges,
            "by_file_class": dict(by_class),
        }

    def task_diff(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        *,
        repository: str | None = None,
        max_bytes: int = 400_000,
    ) -> dict[str, Any]:
        """Full diff text read live from Git (never stored in SQLite)."""
        require(ctx, Permission.GIT_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            repo_filter = (
                resolve.repository(session, task.workspace_id, repository, include_archived=True).id
                if repository
                else None
            )
            surfaces = list(
                session.scalars(select(TaskChangeSurface).where(TaskChangeSurface.task_id == task.id))
            )
            commits = self._task_commits(session, task.id, 0.5)
            repos = {
                r.id: r
                for r in session.scalars(
                    select(Repository).where(Repository.workspace_id == task.workspace_id)
                )
            }
            paths: dict[str, str | None] = {}
            for rid, r in repos.items():
                checkout = primary_checkout(r)
                paths[rid] = checkout.path if checkout else None
            names = {rid: r.name for rid, r in repos.items()}
            task_id, task_key = task.id, task.key
        parts: list[dict[str, Any]] = []
        missing: list[dict[str, str]] = []
        if surfaces:
            for s in surfaces:
                if repo_filter and s.repository_id != repo_filter:
                    continue
                path = paths.get(s.repository_id)
                if not path:
                    continue
                git = Git(path)
                if not (git.object_exists(s.base_sha) and git.object_exists(s.final_sha)):
                    missing.append(
                        {"repository": names[s.repository_id], "range": f"{s.base_sha}..{s.final_sha}"}
                    )
                    continue
                text = git.out("diff", "-M", s.base_sha, s.final_sha)
                parts.append(
                    {
                        "repository": names[s.repository_id],
                        "kind": "final",
                        "range": f"{s.base_sha[:12]}..{s.final_sha[:12]}",
                        "diff": text,
                    }
                )
        else:
            for c in commits:
                if repo_filter and c["repository_id"] != repo_filter:
                    continue
                path = paths.get(c["repository_id"])
                if not path:
                    continue
                git = Git(path)
                if not git.object_exists(c["sha"]):
                    missing.append({"repository": c["repository"], "sha": c["sha"]})
                    continue
                text = git.out("show", "--format=commit %H%n%s%n", "-M", "--first-parent", c["sha"])
                parts.append({"repository": c["repository"], "kind": "commit", "sha": c["sha"], "diff": text})
        if missing:
            self._record_gaps(missing, repos, f"task diff {task_key}")
        total = 0
        truncated = False
        for part in parts:
            remaining = max(0, max_bytes - total)
            if len(part["diff"]) > remaining:
                part["diff"] = part["diff"][:remaining]
                part["truncated"] = True
                truncated = True
            total += len(part["diff"])
        return {
            "task_id": task_id,
            "task_key": task_key,
            "parts": parts,
            "missing_objects": missing,
            "truncated": truncated,
        }

    def _record_gaps(
        self, missing: builtins.list[dict[str, str]], repos: dict[str, Repository], context: str
    ) -> None:
        by_name = {r.name: r.id for r in repos.values()}
        with self.db.session(write=True) as session:
            for item in missing:
                sha = item.get("sha") or item.get("range", "").split("..")[-1]
                repo_id = by_name.get(item["repository"])
                if not repo_id or not sha:
                    continue
                exists = session.scalar(
                    select(GitObjectGap).where(GitObjectGap.repository_id == repo_id, GitObjectGap.sha == sha)
                )
                if exists is None:
                    session.add(GitObjectGap(repository_id=repo_id, sha=sha, context=context))

    def repository_commits(
        self,
        ctx: RequestContext,
        workspace: str | None,
        repository: str,
        *,
        limit: int = 50,
        offset: int = 0,
        linked_only: bool = False,
        include_unreachable: bool = False,
    ) -> dict[str, Any]:
        require(ctx, Permission.GIT_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            repo = resolve.repository(session, ws_id, repository, include_archived=True)
            stmt = select(Commit).where(Commit.repository_id == repo.id)
            if not include_unreachable:
                stmt = stmt.where(Commit.reachable.is_(True))
            if linked_only:
                stmt = stmt.where(Commit.id.in_(select(CommitTaskLink.commit_id)))
            total = int(session.scalar(select(func.count()).select_from(stmt.subquery())) or 0)
            commits = list(
                session.scalars(stmt.order_by(Commit.committed_at.desc()).limit(limit).offset(offset))
            )
            links: dict[str, builtins.list[dict[str, Any]]] = defaultdict(list)
            for link, key in session.execute(
                select(CommitTaskLink, Task.key)
                .join(Task, Task.id == CommitTaskLink.task_id)
                .where(CommitTaskLink.commit_id.in_([c.id for c in commits]))
            ):
                links[link.commit_id].append(
                    {
                        "task_id": link.task_id,
                        "task_key": key,
                        "source": link.source,
                        "confidence": link.confidence,
                    }
                )
            return {
                "repository_id": repo.id,
                "total": total,
                "items": [
                    {
                        "id": c.id,
                        "sha": c.sha,
                        "subject": c.subject,
                        "author": c.author_name,
                        "author_email": c.author_email,
                        "committed_at": c.committed_at.isoformat() if c.committed_at else None,
                        "insertions": c.insertions,
                        "deletions": c.deletions,
                        "files_changed": c.files_changed,
                        "is_merge": c.is_merge,
                        "reachable": c.reachable,
                        "branches": c.branches,
                        "trailers": c.trailers,
                        "tasks": links.get(c.id, []),
                    }
                    for c in commits
                ],
            }

    def commit_detail(
        self, ctx: RequestContext, workspace: str | None, repository: str, sha: str
    ) -> dict[str, Any]:
        require(ctx, Permission.GIT_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            repo = resolve.repository(session, ws_id, repository, include_archived=True)
            commit = session.scalar(
                select(Commit).where(Commit.repository_id == repo.id, Commit.sha.startswith(sha))
            )
            if commit is None:
                raise NotFoundError(f"commit {sha} has not been ingested for {repo.name}")
            files = list(
                session.scalars(select(CommitFileChange).where(CommitFileChange.commit_id == commit.id))
            )
            symbols = defaultdict(list)
            for sym in session.scalars(
                select(CommitSymbol).where(CommitSymbol.file_change_id.in_([f.id for f in files]))
            ):
                symbols[sym.file_change_id].append(
                    {
                        "kind": sym.symbol_kind,
                        "name": sym.name,
                        "change": sym.change_kind,
                        "lines_changed": sym.lines_changed,
                    }
                )
            analysis = session.get(CommitAnalysis, commit.id)
            equivalents = [
                {
                    "kind": e.kind,
                    "original_commit_id": e.original_commit_id,
                    "equivalent_commit_id": e.equivalent_commit_id,
                    "evidence": e.evidence,
                }
                for e in session.scalars(
                    select(CommitEquivalence).where(
                        (CommitEquivalence.original_commit_id == commit.id)
                        | (CommitEquivalence.equivalent_commit_id == commit.id)
                    )
                )
            ]
            links = [
                {
                    "task_id": link.task_id,
                    "task_key": key,
                    "source": link.source,
                    "confidence": link.confidence,
                    "evidence": link.evidence,
                }
                for link, key in session.execute(
                    select(CommitTaskLink, Task.key)
                    .join(Task, Task.id == CommitTaskLink.task_id)
                    .where(CommitTaskLink.commit_id == commit.id)
                )
            ]
            return {
                "id": commit.id,
                "sha": commit.sha,
                "repository_id": repo.id,
                "parents": commit.parents,
                "subject": commit.subject,
                "message": commit.message,
                "trailers": commit.trailers,
                "author": {
                    "name": commit.author_name,
                    "email": commit.author_email,
                    "at": commit.authored_at.isoformat() if commit.authored_at else None,
                },
                "committer": {
                    "name": commit.committer_name,
                    "email": commit.committer_email,
                    "at": commit.committed_at.isoformat() if commit.committed_at else None,
                },
                "is_merge": commit.is_merge,
                "patch_id": commit.patch_id,
                "reachable": commit.reachable,
                "object_available": commit.object_available,
                "branches": commit.branches,
                "insertions": commit.insertions,
                "deletions": commit.deletions,
                "files": [
                    {
                        "path": f.path,
                        "old_path": f.old_path,
                        "change_type": f.change_type,
                        "insertions": f.insertions,
                        "deletions": f.deletions,
                        "binary": f.is_binary,
                        "language": f.language,
                        "file_class": f.file_class,
                        "module": f.module,
                        "symbols": symbols.get(f.id, []),
                    }
                    for f in files
                ],
                "analysis": {
                    "analyzer_version": analysis.analyzer_version,
                    "functions_affected": analysis.functions_affected,
                    "types_affected": analysis.types_affected,
                    "files_parsed": analysis.files_parsed,
                    "files_fallback": analysis.files_fallback,
                    "detail": analysis.detail,
                }
                if analysis
                else None,
                "equivalences": equivalents,
                "tasks": links,
            }

    # ------------------------------------------------------------------ structure

    def analyze_commits(self, commit_ids: builtins.list[str]) -> int:
        analyzed = 0
        for commit_id in commit_ids:
            if self.analyze_commit(commit_id):
                analyzed += 1
        return analyzed

    def analyze_commit(self, commit_id: str) -> bool:
        with self.db.session() as session:
            commit = session.get(Commit, commit_id)
            if commit is None or session.get(CommitAnalysis, commit_id) is not None or commit.is_merge:
                return False
            repo = session.get(Repository, commit.repository_id)
            checkout = primary_checkout(repo) if repo else None
            if checkout is None:
                return False
            files = [
                (f.id, f.path, f.old_path, f.change_type, f.is_binary, f.language)
                for f in session.scalars(
                    select(CommitFileChange).where(CommitFileChange.commit_id == commit_id)
                )
            ]
            sha = commit.sha
            parent = commit.parents[0] if commit.parents else None
        git = Git(checkout.path)
        if not git.object_exists(sha):
            with self.db.session(write=True) as session:
                session.execute(update(Commit).where(Commit.id == commit_id).values(object_available=False))
            return False
        results = []
        parsed = fallback = 0
        for fc_id, path, old_path, status, binary, language in files:
            if binary:
                fallback += 1
                results.append((fc_id, None, "binary"))
                continue
            diff_args = ["diff", "-U0", "--no-color", "-M", parent or EMPTY_TREE, sha]
            diff_args += ["--", *([old_path] if old_path else []), path]
            hunks = git.run(*diff_args, check=False).stdout
            deleted, added = parse_hunks(hunks)
            old_src = git.show_file(parent, old_path or path) if parent and status != "A" else None
            new_src = git.show_file(sha, path) if status != "D" else None
            structure = analyze_file(path, language, old_src, new_src, deleted, added)
            if structure.mode == "structural":
                parsed += 1
            else:
                fallback += 1
            results.append((fc_id, structure, structure.reason))
        with self.db.session(write=True) as session:
            if session.get(CommitAnalysis, commit_id) is not None:
                return False
            functions = types = 0
            detail: dict[str, Any] = {"fallback_reasons": {}}
            for fc_id, structure, reason in results:
                if structure is None or structure.mode != "structural":
                    detail["fallback_reasons"][fc_id] = reason
                    continue
                functions += structure.functions
                types += structure.types
                for change in structure.changes:
                    session.add(
                        CommitSymbol(
                            file_change_id=fc_id,
                            symbol_kind=change.kind,
                            name=change.name[:500],
                            change_kind=change.change,
                            start_line=change.start_line,
                            end_line=change.end_line,
                            lines_changed=change.lines_changed,
                        )
                    )
            session.add(
                CommitAnalysis(
                    commit_id=commit_id,
                    analyzer_version=ANALYZER_VERSION,
                    files_parsed=parsed,
                    files_fallback=fallback,
                    functions_affected=functions,
                    types_affected=types,
                    detail=detail,
                )
            )
            session.execute(update(Commit).where(Commit.id == commit_id).values(structure_analyzed=True))
        return True

    # ------------------------------------------------------------------ final change surface

    def compute_change_surface(self, ctx: RequestContext, task_id: str) -> builtins.list[dict[str, Any]]:
        """Baseline revision versus accepted revision, per repository the task changed."""
        with self.db.session() as session:
            task = session.get(Task, task_id)
            if task is None:
                raise NotFoundError(f"task {task_id} not found")
            acceptance = session.scalar(
                select(TaskAcceptance)
                .where(TaskAcceptance.task_id == task_id, TaskAcceptance.decision == "accepted")
                .order_by(TaskAcceptance.created_at.desc())
                .limit(1)
            )
            final_entries: dict[str, str] = {}
            if acceptance and acceptance.manifest_id:
                manifest = session.get(RevisionManifest, acceptance.manifest_id)
                if manifest:
                    final_entries = {e["repository_id"]: e["sha"] for e in manifest.entries if e.get("sha")}
            first_attempt = session.scalar(
                select(AgentAttempt)
                .where(AgentAttempt.task_id == task_id, AgentAttempt.baseline_manifest_id.is_not(None))
                .order_by(AgentAttempt.number.asc())
                .limit(1)
            )
            base_entries: dict[str, str] = {}
            if first_attempt is not None:
                baseline = session.get(RevisionManifest, first_attempt.baseline_manifest_id)
                if baseline:
                    base_entries = {e["repository_id"]: e["sha"] for e in baseline.entries if e.get("sha")}
            commits = self._task_commits(session, task_id, 0.5)
            repos = {
                r.id: r
                for r in session.scalars(
                    select(Repository).where(Repository.workspace_id == task.workspace_id)
                )
            }
            checkout_paths: dict[str, str] = {}
            for rid, r in repos.items():
                co = primary_checkout(r)
                if co is not None:
                    checkout_paths[rid] = co.path
            attempt_id = acceptance.attempt_id if acceptance else None
        by_repo: dict[str, builtins.list[dict[str, Any]]] = defaultdict(list)
        for c in commits:
            if not c["is_merge"]:
                by_repo[c["repository_id"]].append(c)
        repo_ids = set(final_entries) | set(by_repo)
        surfaces: list[dict[str, Any]] = []
        for repo_id in repo_ids:
            repo = repos.get(repo_id)
            checkout_path = checkout_paths.get(repo_id)
            if checkout_path is None:
                continue
            git = Git(checkout_path)
            linked = by_repo.get(repo_id, [])
            base = base_entries.get(repo_id)
            final = final_entries.get(repo_id)
            provenance = {
                "base": "attempt baseline manifest" if base else None,
                "final": "accepted revision manifest" if final else None,
            }
            if (base is None or final is None) and linked:
                shas = [c["sha"] for c in linked if git.object_exists(c["sha"])]
                if not shas:
                    continue
                if final is None:
                    final = next((s for s in shas if all(git.is_ancestor(o, s) for o in shas)), shas[-1])
                    provenance["final"] = "latest linked commit"
                if base is None:
                    earliest = next((s for s in shas if all(git.is_ancestor(s, o) for o in shas)), shas[0])
                    parent = git.run(
                        "rev-parse", "--verify", "--quiet", f"{earliest}^", check=False
                    ).stdout.strip()
                    base = parent or EMPTY_TREE
                    provenance["base"] = "parent of earliest linked commit"
            if not base or not final or base == final:
                continue
            if not (git.object_exists(final) and (git.object_exists(base) or base == EMPTY_TREE)):
                self._record_gaps(
                    [{"repository": repo.name if repo else repo_id, "range": f"{base}..{final}"}],
                    repos,
                    f"change surface {task_id}",
                )
                continue
            numstat = parse_numstat_z(git.out("diff", "--numstat", "-z", "-M", base, final))
            functions = types = 0
            fallback_files = 0
            for f in numstat:
                if f.binary:
                    continue
                language = language_for(f.path)
                hunks = git.run(
                    "diff",
                    "-U0",
                    "--no-color",
                    "-M",
                    base,
                    final,
                    "--",
                    *([f.old_path] if f.old_path else []),
                    f.path,
                    check=False,
                ).stdout
                deleted, added = parse_hunks(hunks)
                old_src = git.show_file(base, f.old_path or f.path)
                new_src = git.show_file(final, f.path)
                structure = analyze_file(f.path, language, old_src, new_src, deleted, added)
                if structure.mode == "structural":
                    functions += structure.functions
                    types += structure.types
                else:
                    fallback_files += 1
            record = {
                "repository_id": repo_id,
                "base_sha": base,
                "final_sha": final,
                "files_changed": len(numstat),
                "insertions": sum(f.insertions for f in numstat),
                "deletions": sum(f.deletions for f in numstat),
                "renames": sum(1 for f in numstat if f.old_path),
                "binary_files": sum(1 for f in numstat if f.binary),
                "functions_affected": functions,
                "types_affected": types,
                "detail": {
                    "provenance": provenance,
                    "structural_fallback_files": fallback_files,
                    "files": [
                        {
                            "path": f.path,
                            "old_path": f.old_path,
                            "insertions": f.insertions,
                            "deletions": f.deletions,
                            "binary": f.binary,
                            "class": file_class(f.path),
                        }
                        for f in numstat[:500]
                    ],
                },
            }
            surfaces.append(record)
        with self.db.session(write=True) as session:
            for record in surfaces:
                existing = session.scalar(
                    select(TaskChangeSurface).where(
                        TaskChangeSurface.task_id == task_id,
                        TaskChangeSurface.repository_id == record["repository_id"],
                    )
                )
                if existing is None:
                    session.add(TaskChangeSurface(task_id=task_id, attempt_id=attempt_id, **record))
                else:
                    for key, value in record.items():
                        setattr(existing, key, value)
                    existing.attempt_id = attempt_id
            if surfaces:
                record_event(
                    session,
                    ctx,
                    event_type="task.change_surface_computed",
                    entity_type="task",
                    entity_id=task_id,
                    workspace_id=task.workspace_id,
                    data={
                        "repositories": len(surfaces),
                        "insertions": sum(s["insertions"] for s in surfaces),
                        "deletions": sum(s["deletions"] for s in surfaces),
                    },
                )
        return surfaces

    # ------------------------------------------------------------------ change sets

    def change_sets(
        self, ctx: RequestContext, workspace: str | None, task_ref: str
    ) -> builtins.list[dict[str, Any]]:
        require(ctx, Permission.GIT_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            out = []
            for cs in session.scalars(
                select(ChangeSet).where(ChangeSet.task_id == task.id).order_by(ChangeSet.created_at)
            ):
                items = session.scalars(select(ChangeSetItem).where(ChangeSetItem.change_set_id == cs.id))
                out.append(
                    {
                        "id": cs.id,
                        "attempt_id": cs.attempt_id,
                        "title": cs.title,
                        "status": cs.status,
                        "created_at": cs.created_at.isoformat(),
                        "items": [
                            {
                                "repository_id": i.repository_id,
                                "base_sha": i.base_sha,
                                "head_sha": i.head_sha,
                                "branch": i.branch,
                                "commits": i.commit_shas,
                                "status": i.status,
                                "detail": i.detail,
                            }
                            for i in items
                        ],
                    }
                )
            return out
