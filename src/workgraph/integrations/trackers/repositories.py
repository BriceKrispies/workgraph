"""Resolve tracker pull requests to the repositories registered in a workspace."""

from __future__ import annotations

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from workgraph.persistence.orm import Repository, RepositoryRemote


def resolve_repository(session: Session, workspace_id: str, repo_normalized_url: str) -> str | None:
    """Id of the non-archived repository in ``workspace_id`` whose remote matches, or None.

    None also when several repositories match: the link then stays unresolved rather than guessing.
    """
    ids = set(
        session.scalars(
            select(Repository.id)
            .join(RepositoryRemote, RepositoryRemote.repository_id == Repository.id)
            .where(
                Repository.workspace_id == workspace_id,
                Repository.archived_at.is_(None),
                func.lower(RepositoryRemote.normalized_url) == repo_normalized_url.lower(),
            )
        )
    )
    if len(ids) != 1:
        return None
    return ids.pop()
