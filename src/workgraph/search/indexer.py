"""Maintains the SQLite FTS5 index (``task_fts``) inside the caller's transaction."""

from __future__ import annotations

import re

from sqlalchemy import select, text
from sqlalchemy.orm import Session

from workgraph.persistence.orm import Classification, Task, TaskComment, TaskTag


def index_task(session: Session, task_id: str) -> None:
    task = session.get(Task, task_id)
    session.execute(text("DELETE FROM task_fts WHERE task_id = :id"), {"id": task_id})
    if task is None:
        return
    comments = session.scalars(
        select(TaskComment.body).where(TaskComment.task_id == task_id).order_by(TaskComment.created_at)
    ).all()
    tags = session.scalars(select(TaskTag.tag).where(TaskTag.task_id == task_id)).all()
    summary_parts: list[str] = []
    if task.current_classification_id:
        cls = session.get(Classification, task.current_classification_id)
        if cls is not None:
            summary_parts.extend(
                filter(None, [cls.category, cls.subtype, cls.domain, *cls.affected_systems, *cls.tags])
            )
    session.execute(
        text(
            "INSERT INTO task_fts (task_id, workspace_id, key, title, description, comments, tags, summary) "
            "VALUES (:id, :ws, :key, :title, :description, :comments, :tags, :summary)"
        ),
        {
            "id": task.id,
            "ws": task.workspace_id,
            "key": task.key,
            "title": task.title,
            "description": task.description,
            "comments": "\n".join(comments),
            "tags": " ".join(tags),
            "summary": " ".join(summary_parts),
        },
    )


_TOKEN_RE = re.compile(r"[\w]+", re.UNICODE)


def to_fts_query(user_query: str, *, prefix: bool = True, match_any: bool = False) -> str | None:
    """Turn free text into a safe FTS5 MATCH expression (no operator injection).

    Each token is quoted; the final token gets a prefix wildcard to support
    search-as-you-type. Tokens are AND-ed together unless ``match_any``.
    """
    tokens = _TOKEN_RE.findall(user_query)
    if not tokens:
        return None
    quoted = [f'"{t}"' for t in tokens]
    if prefix:
        quoted[-1] = quoted[-1] + "*"
    if match_any:
        # Similarity lookups: any shared term counts; bm25 ranks documents sharing more terms higher.
        return " OR ".join(dict.fromkeys(quoted[:40]))
    return " ".join(quoted)
