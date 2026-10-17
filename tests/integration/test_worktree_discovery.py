from __future__ import annotations

from sqlalchemy import select

from workgraph.core.models.schemas import RepositoryRegister
from workgraph.persistence.orm import Event


def register(app, human, ws, repo):
    return app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))


def event_types(app, entity_id: str) -> list[str]:
    with app.db.session() as session:
        return list(session.scalars(select(Event.event_type).where(Event.entity_id == entity_id)))


def test_register_and_refresh_discover_worktrees(app, human, ws, make_repo, tmp_path):
    repo = make_repo("discover")
    wt_a = tmp_path / "wt" / "feature-a"
    wt_b = tmp_path / "wt" / "detached-b"
    repo.git("worktree", "add", "-q", "-b", "feature-a", str(wt_a))
    repo.git("worktree", "add", "-q", "--detach", str(wt_b))
    repo.git("worktree", "lock", str(wt_b))

    out = register(app, human, ws, repo)
    assert len(app.repositories.list(human, ws)) == 1
    assert {c.path: c.kind for c in out.checkouts} == {
        str(repo.path.resolve()): "clone",
        str(wt_a.resolve()): "worktree",
        str(wt_b.resolve()): "worktree",
    }
    assert out.health_detail["skipped_worktrees"] == []
    assert event_types(app, out.id).count("repository.checkout_discovered") == 2

    wt_c = tmp_path / "wt" / "feature-c"
    repo.git("worktree", "add", "-q", "-b", "feature-c", str(wt_c))
    repo.git("worktree", "remove", str(wt_a))
    refreshed = app.repositories.refresh(human, ws, "discover")
    by_path = {c.path: c for c in refreshed.checkouts}
    assert len(by_path) == 4
    assert by_path[str(wt_c.resolve())].kind == "worktree"
    assert by_path[str(wt_c.resolve())].available is True
    assert by_path[str(wt_a.resolve())].available is False
    assert by_path[str(wt_b.resolve())].available is True
    assert refreshed.health_status == "degraded"
    types = event_types(app, out.id)
    assert types.count("repository.checkout_lost") == 1
    assert types.count("repository.checkout_discovered") == 3


def test_managed_worktrees_are_not_registered_as_user_checkouts(app, human, ws, make_repo):
    repo = make_repo("managed")
    managed = app.paths.worktrees_dir / "managed-wt"
    repo.git("worktree", "add", "-q", "--detach", str(managed))

    out = register(app, human, ws, repo)
    assert [c.path for c in out.checkouts] == [str(repo.path.resolve())]
    refreshed = app.repositories.refresh(human, ws, "managed")
    assert [c.path for c in refreshed.checkouts] == [str(repo.path.resolve())]


def test_worktrees_outside_allowed_roots_are_skipped_and_reported(app, human, ws, make_repo, tmp_path):
    repo = make_repo("bounded")
    outside = tmp_path / "elsewhere" / "wt"
    repo.git("worktree", "add", "-q", "--detach", str(outside))
    app.workspaces.update_settings(human, ws, {"allowed_roots": [str(repo.path)]})

    out = register(app, human, ws, repo)
    assert [c.path for c in out.checkouts] == [str(repo.path.resolve())]
    assert [s["path"] for s in out.health_detail["skipped_worktrees"]] == [str(outside.resolve())]
