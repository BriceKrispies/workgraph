"""Lexical (FTS5), semantic (embeddings) and hybrid task search.

Full-text search always works. Semantic search requires embeddings produced by the
configured embedding provider/model; vectors are stored per (provider, model), so
changing models never mixes incompatible vector spaces and embeddings can be rebuilt.
Hybrid retrieval fuses both rankings with reciprocal rank fusion (RRF).
"""

from __future__ import annotations

import builtins
import hashlib
import math
from array import array
from datetime import datetime
from typing import Any

from pydantic import BaseModel, Field
from sqlalchemy import Select, func, select, text
from sqlalchemy.orm import Session

from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError, ProviderUnavailableError, WorkgraphError
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.jobs import queue as jobqueue
from workgraph.persistence.orm import (
    AgentAttempt,
    Commit,
    CommitFileChange,
    CommitTaskLink,
    Task,
    TaskEmbedding,
    TaskProject,
    TaskRepository,
)
from workgraph.search.indexer import to_fts_query
from workgraph.security.permissions import Permission, require

RRF_K = 60


class SearchFilters(BaseModel):
    project: str | None = None
    repository: str | None = None
    category: str | None = None
    statuses: list[str] | None = None
    created_after: datetime | None = None
    created_before: datetime | None = None
    agent: str | None = Field(
        default=None, description="Attempt provider (agent adapter) that worked on the task"
    )
    model: str | None = None
    changed_path: str | None = Field(
        default=None, description="Substring of a file path changed by linked commits"
    )
    include_archived: bool = False


class SearchHit(BaseModel):
    task_id: str
    key: str
    title: str
    status: str
    category: str | None
    score: float
    lexical_rank: int | None = None
    semantic_rank: int | None = None
    similarity: float | None = None
    snippet: str | None = None


class SearchResult(BaseModel):
    query: str
    mode: str
    semantic_available: bool
    embedding_model: str | None
    hits: list[SearchHit]
    note: str | None = None


def pack(vector: builtins.list[float]) -> bytes:
    return array("f", vector).tobytes()


def unpack(blob: bytes) -> builtins.list[float]:
    values = array("f")
    values.frombytes(blob)
    return values.tolist()


def cosine(a: builtins.list[float], b: builtins.list[float]) -> float:
    if len(a) != len(b):
        return 0.0
    dot = sum(x * y for x, y in zip(a, b, strict=True))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(y * y for y in b))
    return dot / (na * nb) if na and nb else 0.0


def embedding_text(task: Task) -> str:
    parts = [
        task.title,
        task.description or "",
        *(task.acceptance_criteria or []),
        " ".join(t.tag for t in task.tags),
    ]
    return "\n".join(p for p in parts if p)[:8000]


class SearchService(Service):
    def _scope(self, session: Session, ws_id: str, flt: SearchFilters) -> Select[Any]:
        stmt = select(Task.id).where(Task.workspace_id == ws_id)
        if not flt.include_archived:
            stmt = stmt.where(Task.archived_at.is_(None))
        if flt.project:
            proj = resolve.project(session, ws_id, flt.project)
            stmt = stmt.where(
                Task.id.in_(select(TaskProject.task_id).where(TaskProject.project_id == proj.id))
            )
        if flt.repository:
            repo = resolve.repository(session, ws_id, flt.repository, include_archived=True)
            stmt = stmt.where(
                Task.id.in_(select(TaskRepository.task_id).where(TaskRepository.repository_id == repo.id))
            )
        if flt.category:
            stmt = stmt.where(Task.category == flt.category)
        if flt.statuses:
            stmt = stmt.where(Task.status.in_(flt.statuses))
        if flt.created_after:
            stmt = stmt.where(Task.created_at >= flt.created_after)
        if flt.created_before:
            stmt = stmt.where(Task.created_at < flt.created_before)
        if flt.agent:
            stmt = stmt.where(
                Task.id.in_(select(AgentAttempt.task_id).where(AgentAttempt.provider == flt.agent))
            )
        if flt.model:
            stmt = stmt.where(
                Task.id.in_(select(AgentAttempt.task_id).where(AgentAttempt.model == flt.model))
            )
        if flt.changed_path:
            stmt = stmt.where(
                Task.id.in_(
                    select(CommitTaskLink.task_id)
                    .join(CommitFileChange, CommitFileChange.commit_id == CommitTaskLink.commit_id)
                    .where(CommitFileChange.path.contains(flt.changed_path))
                )
            )
        return stmt

    # ------------------------------------------------------------------ lexical

    def lexical(
        self,
        ctx: RequestContext,
        workspace: str | None,
        query: str,
        *,
        filters: SearchFilters | None = None,
        limit: int = 20,
        match_any: bool = False,
    ) -> builtins.list[SearchHit]:
        require(ctx, Permission.TASK_READ)
        flt = filters or SearchFilters()
        match = to_fts_query(query, match_any=match_any)
        if match is None:
            return []
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            scope_ids = set(session.scalars(self._scope(session, ws_id, flt)))
            rows = session.execute(
                text(
                    "SELECT task_id, bm25(task_fts, 0, 0, 5.0, 10.0, 3.0, 1.0, 2.0, 2.0) AS rank, "
                    "snippet(task_fts, -1, '[', ']', ' ... ', 12) AS snip "
                    "FROM task_fts WHERE task_fts MATCH :m AND workspace_id = :ws ORDER BY rank LIMIT :lim"
                ),
                {"m": match, "ws": ws_id, "lim": limit * 5},
            ).all()
            hits: list[SearchHit] = []
            for task_id, rank, snip in rows:
                if task_id not in scope_ids:
                    continue
                task = session.get(Task, task_id)
                if task is None:
                    continue
                hits.append(
                    SearchHit(
                        task_id=task.id,
                        key=task.key,
                        title=task.title,
                        status=task.status,
                        category=task.category,
                        score=round(-float(rank), 4),
                        lexical_rank=len(hits) + 1,
                        snippet=snip,
                    )
                )
                if len(hits) >= limit:
                    break
            return hits

    # ------------------------------------------------------------------ embeddings

    def embed_task(
        self, task_id: str, *, provider: str | None = None, model: str | None = None
    ) -> dict[str, Any]:
        with self.db.session() as session:
            task = session.get(Task, task_id)
            if task is None:
                raise NotFoundError(f"task {task_id} not found")
            slug = self.app.workspace_slug(task.workspace_id)
            content = embedding_text(task)
        selection = self.app.llm.selection("embeddings", slug)
        provider_name, model_name = provider or selection.provider, model or selection.model
        digest = hashlib.sha256(content.encode("utf-8")).hexdigest()
        with self.db.session() as session:
            existing = session.scalar(
                select(TaskEmbedding).where(
                    TaskEmbedding.task_id == task_id,
                    TaskEmbedding.provider == provider_name,
                    TaskEmbedding.model == model_name,
                )
            )
            if existing is not None and existing.input_hash == digest:
                return {
                    "task_id": task_id,
                    "status": "unchanged",
                    "provider": provider_name,
                    "model": model_name,
                }
        vector = self.app.llm.get(provider_name, slug).embed(model_name, [content])[0]
        with self.db.session(write=True) as session:
            existing = session.scalar(
                select(TaskEmbedding).where(
                    TaskEmbedding.task_id == task_id,
                    TaskEmbedding.provider == provider_name,
                    TaskEmbedding.model == model_name,
                )
            )
            if existing is None:
                session.add(
                    TaskEmbedding(
                        task_id=task_id,
                        provider=provider_name,
                        model=model_name,
                        dimensions=len(vector),
                        vector=pack(vector),
                        input_hash=digest,
                    )
                )
            else:
                existing.vector, existing.dimensions, existing.input_hash = pack(vector), len(vector), digest
        return {
            "task_id": task_id,
            "status": "embedded",
            "provider": provider_name,
            "model": model_name,
            "dimensions": len(vector),
        }

    def rebuild_embeddings(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        provider: str | None = None,
        model: str | None = None,
    ) -> dict[str, Any]:
        require(ctx, Permission.CLASSIFY_REQUEST)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            ids = list(session.scalars(select(Task.id).where(Task.workspace_id == ws_id)))
            for task_id in ids:
                jobqueue.enqueue(
                    session,
                    "embed_task",
                    {"task_id": task_id, "provider": provider, "model": model},
                    workspace_id=ws_id,
                    task_id=task_id,
                    resource_keys=["gpu"],
                    max_attempts=3,
                    idempotency_key=f"embed-rebuild:{task_id}:{provider}:{model}:{len(ids)}:{ws_id[:6]}",
                    created_by=ctx.actor,
                )
        return {"enqueued": len(ids), "provider": provider, "model": model}

    def _vectors(
        self, session: Session, ws_id: str, provider: str, model: str
    ) -> dict[str, builtins.list[float]]:
        rows = session.execute(
            select(TaskEmbedding.task_id, TaskEmbedding.vector)
            .join(Task, Task.id == TaskEmbedding.task_id)
            .where(
                Task.workspace_id == ws_id, TaskEmbedding.provider == provider, TaskEmbedding.model == model
            )
        ).all()
        return {task_id: unpack(blob) for task_id, blob in rows}

    def semantic(
        self,
        ctx: RequestContext,
        workspace: str | None,
        query: str,
        *,
        filters: SearchFilters | None = None,
        limit: int = 20,
    ) -> tuple[builtins.list[SearchHit], str | None]:
        """Returns hits and the embedding model used, or ([], reason) if embeddings are unavailable."""
        require(ctx, Permission.TASK_READ)
        flt = filters or SearchFilters()
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            slug = self.app.workspace_slug(ws_id)
            selection = self.app.llm.selection("embeddings", slug)
            vectors = self._vectors(session, ws_id, selection.provider, selection.model)
            scope_ids = set(session.scalars(self._scope(session, ws_id, flt)))
        if not vectors:
            raise ProviderUnavailableError(
                f"no embeddings stored for {selection.provider}/{selection.model}; run `workgraph search reindex`"
            )
        query_vector = self.app.llm.get(selection.provider, slug).embed(selection.model, [query])[0]
        scored = sorted(
            ((cosine(query_vector, v), tid) for tid, v in vectors.items() if tid in scope_ids), reverse=True
        )
        hits = []
        with self.db.session() as session:
            for sim, tid in scored[:limit]:
                task = session.get(Task, tid)
                if task is None:
                    continue
                hits.append(
                    SearchHit(
                        task_id=tid,
                        key=task.key,
                        title=task.title,
                        status=task.status,
                        category=task.category,
                        score=round(sim, 4),
                        similarity=round(sim, 4),
                        semantic_rank=len(hits) + 1,
                    )
                )
        return hits, f"{selection.provider}/{selection.model}"

    def search(
        self,
        ctx: RequestContext,
        workspace: str | None,
        query: str,
        *,
        mode: str = "hybrid",
        filters: SearchFilters | None = None,
        limit: int = 20,
    ) -> SearchResult:
        lexical = (
            self.lexical(ctx, workspace, query, filters=filters, limit=limit * 2)
            if mode in ("hybrid", "lexical")
            else []
        )
        semantic: list[SearchHit] = []
        model_used: str | None = None
        note = None
        if mode in ("hybrid", "semantic"):
            try:
                semantic, model_used = self.semantic(ctx, workspace, query, filters=filters, limit=limit * 2)
            except (ProviderUnavailableError, NotFoundError, WorkgraphError) as exc:
                note = f"semantic search unavailable: {exc.message}; lexical results only"
                if mode == "semantic":
                    return SearchResult(
                        query=query,
                        mode=mode,
                        semantic_available=False,
                        embedding_model=None,
                        hits=[],
                        note=note,
                    )
        if mode == "lexical" or not semantic:
            return SearchResult(
                query=query,
                mode="lexical" if mode != "semantic" else mode,
                semantic_available=bool(semantic),
                embedding_model=model_used,
                hits=lexical[:limit],
                note=note,
            )
        if mode == "semantic":
            return SearchResult(
                query=query,
                mode=mode,
                semantic_available=True,
                embedding_model=model_used,
                hits=semantic[:limit],
            )
        fused: dict[str, SearchHit] = {}
        scores: dict[str, float] = {}
        for rank, hit in enumerate(lexical, start=1):
            scores[hit.task_id] = scores.get(hit.task_id, 0.0) + 1.0 / (RRF_K + rank)
            fused[hit.task_id] = hit.model_copy(update={"lexical_rank": rank})
        for rank, hit in enumerate(semantic, start=1):
            scores[hit.task_id] = scores.get(hit.task_id, 0.0) + 1.0 / (RRF_K + rank)
            base = fused.get(hit.task_id, hit)
            fused[hit.task_id] = base.model_copy(update={"semantic_rank": rank, "similarity": hit.similarity})
        ordered = sorted(fused.values(), key=lambda h: -scores[h.task_id])[:limit]
        hits = [h.model_copy(update={"score": round(scores[h.task_id], 6)}) for h in ordered]
        return SearchResult(
            query=query,
            mode="hybrid",
            semantic_available=True,
            embedding_model=model_used,
            hits=hits,
            note="score = reciprocal rank fusion of lexical (bm25) and semantic (cosine) rankings",
        )

    # ------------------------------------------------------------------ similarity

    def similar(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        *,
        limit: int = 10,
        completed_only: bool = False,
    ) -> dict[str, Any]:
        """Historically comparable tasks, with their linked implementations as evidence."""
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            ws_id = task.workspace_id
            slug = self.app.workspace_slug(ws_id)
            selection = self.app.llm.selection("embeddings", slug)
            vectors = self._vectors(session, ws_id, selection.provider, selection.model)
            my_repos = set(
                session.scalars(select(TaskRepository.repository_id).where(TaskRepository.task_id == task.id))
            )
            query_text = f"{task.title} {task.description[:500]}"
            target_id, category = task.id, task.category
        candidates: dict[str, dict[str, Any]] = {}
        method = "lexical"
        if target_id in vectors:
            method = "semantic"
            mine = vectors[target_id]
            for tid, vec in vectors.items():
                if tid != target_id:
                    candidates[tid] = {"similarity": cosine(mine, vec)}
        else:
            for rank, hit in enumerate(
                self.lexical(ctx, slug, query_text, limit=50, match_any=True), start=1
            ):
                if hit.task_id != target_id:
                    candidates[hit.task_id] = {"similarity": round(1.0 / rank, 4)}
        out: list[dict[str, Any]] = []
        with self.db.session() as session:
            for tid, info in candidates.items():
                other = session.get(Task, tid)
                if other is None or (completed_only and other.status != "accepted"):
                    continue
                repos = set(
                    session.scalars(select(TaskRepository.repository_id).where(TaskRepository.task_id == tid))
                )
                overlap = len(repos & my_repos) / len(repos | my_repos) if repos | my_repos else 0.0
                score = (
                    info["similarity"]
                    + (0.1 if category and other.category == category else 0.0)
                    + 0.1 * overlap
                )
                commits = session.execute(
                    select(Commit.sha, Commit.subject, Commit.repository_id)
                    .join(CommitTaskLink, CommitTaskLink.commit_id == Commit.id)
                    .where(CommitTaskLink.task_id == tid, CommitTaskLink.confidence >= 0.5)
                    .limit(5)
                ).all()
                out.append(
                    {
                        "task_id": tid,
                        "key": other.key,
                        "title": other.title,
                        "status": other.status,
                        "category": other.category,
                        "score": round(score, 4),
                        "similarity": round(info["similarity"], 4),
                        "same_category": bool(category and other.category == category),
                        "repository_overlap": round(overlap, 3),
                        "implementation": [
                            {"sha": c.sha, "subject": c.subject, "repository_id": c.repository_id}
                            for c in commits
                        ],
                        "attempts": int(
                            session.scalar(select(func.count()).where(AgentAttempt.task_id == tid)) or 0
                        ),
                    }
                )
        out.sort(key=lambda r: -float(r["score"]))
        return {
            "task_id": target_id,
            "method": method,
            "embedding_model": f"{selection.provider}/{selection.model}" if method == "semantic" else None,
            "similar": out[:limit],
        }
