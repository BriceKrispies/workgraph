"""Resolving tracker pull request repositories against the registered repositories of a workspace."""

from __future__ import annotations

from datetime import UTC, datetime

import pytest
from sqlalchemy import select
from sqlalchemy.orm import Session

from workgraph.app import Workgraph
from workgraph.core.context import RequestContext
from workgraph.core.models.schemas import WorkspaceCreate
from workgraph.integrations.trackers.repositories import resolve_repository
from workgraph.persistence.orm import Repository, RepositoryRemote, Workspace


def _workspace_id(session: Session, slug: str) -> str:
    workspace_id = session.scalar(select(Workspace.id).where(Workspace.slug == slug))
    assert workspace_id is not None
    return workspace_id


@pytest.fixture
def add_repo(app: Workgraph):
    """Insert a repository with the given remotes, as ``(name, normalized_url)`` pairs."""

    def add(
        workspace_slug: str,
        name: str,
        remotes: list[tuple[str, str]],
        *,
        archived: bool = False,
    ) -> str:
        with app.db.session(write=True) as session:
            repo = Repository(
                workspace_id=_workspace_id(session, workspace_slug),
                name=name,
                archived_at=datetime.now(UTC) if archived else None,
            )
            repo.remotes = [
                RepositoryRemote(name=remote_name, url=f"https://{normalized}.git", normalized_url=normalized)
                for remote_name, normalized in remotes
            ]
            session.add(repo)
            session.flush()
            return repo.id

    return add


def _resolve(app: Workgraph, workspace_slug: str, normalized: str) -> str | None:
    with app.db.session() as session:
        return resolve_repository(session, _workspace_id(session, workspace_slug), normalized)


def test_resolves_the_repository_whose_remote_matches(app, human: RequestContext, ws, add_repo) -> None:
    repo_id = add_repo(ws, "svc-a", [("origin", "github.com/acme/svc-a")])
    add_repo(ws, "svc-b", [("origin", "github.com/acme/svc-b")])
    assert _resolve(app, ws, "github.com/acme/svc-a") == repo_id


def test_match_is_case_insensitive(app, human: RequestContext, ws, add_repo) -> None:
    repo_id = add_repo(ws, "svc-a", [("origin", "github.com/acme/svc-a")])
    assert _resolve(app, ws, "GitHub.com/Acme/Svc-A") == repo_id


def test_unknown_repository_resolves_to_none(app, human: RequestContext, ws, add_repo) -> None:
    add_repo(ws, "svc-a", [("origin", "github.com/acme/svc-a")])
    assert _resolve(app, ws, "github.com/acme/svc-missing") is None


def test_archived_repository_is_not_resolved(app, human: RequestContext, ws, add_repo) -> None:
    add_repo(ws, "svc-old", [("origin", "github.com/acme/svc-a")], archived=True)
    assert _resolve(app, ws, "github.com/acme/svc-a") is None


def test_archived_match_does_not_hide_an_active_one(app, human: RequestContext, ws, add_repo) -> None:
    add_repo(ws, "svc-old", [("origin", "github.com/acme/svc-a")], archived=True)
    active = add_repo(ws, "svc-a", [("origin", "github.com/acme/svc-a")])
    assert _resolve(app, ws, "github.com/acme/svc-a") == active


def test_ambiguous_match_resolves_to_none(app, human: RequestContext, ws, add_repo) -> None:
    add_repo(ws, "svc-a", [("origin", "github.com/acme/svc-a")])
    add_repo(ws, "svc-a-mirror", [("origin", "github.com/acme/svc-a")])
    assert _resolve(app, ws, "github.com/acme/svc-a") is None


def test_one_repository_with_two_matching_remotes_is_not_ambiguous(
    app, human: RequestContext, ws, add_repo
) -> None:
    repo_id = add_repo(
        ws, "svc-a", [("origin", "github.com/acme/svc-a"), ("upstream", "github.com/acme/svc-a")]
    )
    assert _resolve(app, ws, "github.com/acme/svc-a") == repo_id


def test_other_workspaces_are_not_searched(app: Workgraph, human: RequestContext, ws, add_repo) -> None:
    app.workspaces.create(human, WorkspaceCreate(slug="work", name="Work", key_prefix="WK"))
    add_repo("work", "svc-a", [("origin", "github.com/acme/svc-a")])
    assert _resolve(app, ws, "github.com/acme/svc-a") is None
    assert _resolve(app, "work", "github.com/acme/svc-a") is not None
