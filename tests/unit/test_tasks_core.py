from __future__ import annotations

import pytest
from sqlalchemy import text

from workgraph.core.errors import (
    ConcurrencyError,
    ConflictError,
    DependencyCycleError,
    InvalidTransitionError,
    NotFoundError,
    PermissionDenied,
    ResourceBusyError,
)
from workgraph.core.models.schemas import (
    BulkUpdate,
    ClaimRequest,
    CommentCreate,
    ProjectCreate,
    RelationCreate,
    TaskCreate,
    TaskFilter,
    TaskUpdate,
    WorkspaceCreate,
)


def test_create_and_get_task_with_key(app, human, ws):
    t1 = app.tasks.create(
        human, ws, TaskCreate(title="First", description="Hello **world**", tags=["a", "b"])
    )
    t2 = app.tasks.create(human, ws, TaskCreate(title="Second"))
    assert t1.key == "WG-1"
    assert t2.key == "WG-2"
    assert t1.status == "backlog"
    got = app.tasks.get(human, ws, "wg-1")
    assert got.id == t1.id
    assert got.tags == ["a", "b"]
    assert app.tasks.get(human, ws, t1.id).key == "WG-1"


def test_status_transitions_are_validated(app, human, ws):
    t = app.tasks.create(human, ws, TaskCreate(title="T", status="ready"))
    assert t.status == "ready"
    with pytest.raises(InvalidTransitionError):
        app.tasks.update(human, ws, t.key, TaskUpdate(status="accepted"))
    with pytest.raises(InvalidTransitionError):
        app.tasks.update(human, ws, t.key, TaskUpdate(status="awaiting_review"))
    out = app.tasks.update(human, ws, t.key, TaskUpdate(status="cancelled"))
    assert out.status == "cancelled"


def test_optimistic_concurrency(app, human, ws):
    t = app.tasks.create(human, ws, TaskCreate(title="T"))
    updated = app.tasks.update(human, ws, t.key, TaskUpdate(expected_version=t.version, title="T2"))
    assert updated.version == t.version + 1
    with pytest.raises(ConcurrencyError):
        app.tasks.update(human, ws, t.key, TaskUpdate(expected_version=t.version, title="stale"))


def test_dependencies_block_and_unblock(app, human, ws):
    a = app.tasks.create(human, ws, TaskCreate(title="A", status="ready"))
    b = app.tasks.create(human, ws, TaskCreate(title="B", status="ready", depends_on=[a.key]))
    assert b.status == "blocked"
    ready_keys = [t.key for t in app.tasks.get_ready(human, ws)]
    assert ready_keys == [a.key]
    # A claimed blocker does not satisfy the dependency.
    app.tasks.claim(human, ws, a.key, ClaimRequest())
    assert app.tasks.get(human, ws, b.key).status == "blocked"
    # Completion (awaiting review) does satisfy it; acceptance of a is still required for b to be accepted.
    app.tasks.complete(human, ws, a.key, summary="done")
    assert app.tasks.get(human, ws, b.key).status == "ready"
    app.verification.accept_manually(human, ws, a.key, reason="reviewed by hand")
    assert app.tasks.get(human, ws, a.key).status == "accepted"
    assert app.tasks.get(human, ws, b.key).status == "ready"


def test_completed_blocker_satisfies_claim_and_complete(app, human, ws):
    a = app.tasks.create(human, ws, TaskCreate(title="A", status="ready"))
    b = app.tasks.create(human, ws, TaskCreate(title="B", status="ready", depends_on=[a.key]))
    app.tasks.claim(human, ws, a.key, ClaimRequest())
    # Blocker still claimed: b cannot be claimed.
    with pytest.raises(InvalidTransitionError):
        app.tasks.claim(human, ws, b.key, ClaimRequest())
    app.tasks.complete(human, ws, a.key, summary="done")
    # Blocker awaiting review: b is unblocked, can be claimed and completed.
    assert app.tasks.get(human, ws, b.key).status == "ready"
    claimed = app.tasks.claim(human, ws, b.key, ClaimRequest())
    assert claimed.status == "claimed"
    done = app.tasks.complete(human, ws, b.key, summary="done")
    assert done.status == "awaiting_review"
    # Acceptance of b refuses while its blocker a is not accepted.
    with pytest.raises(ConflictError):
        app.verification.accept_manually(human, ws, b.key, reason="too early")
    assert app.tasks.get(human, ws, b.key).status == "awaiting_review"
    app.verification.accept_manually(human, ws, a.key, reason="reviewed by hand")
    app.verification.accept_manually(human, ws, b.key, reason="reviewed by hand")
    assert app.tasks.get(human, ws, b.key).status == "accepted"


def test_accept_refuses_while_blocker_is_only_awaiting_review(app, human, ws):
    a = app.tasks.create(human, ws, TaskCreate(title="A", status="ready"))
    b = app.tasks.create(human, ws, TaskCreate(title="B", status="ready", depends_on=[a.key]))
    _accept_ready_task(app, human, ws, a.key)
    app.tasks.claim(human, ws, b.key, ClaimRequest())
    app.tasks.complete(human, ws, b.key, summary="done")
    c = app.tasks.create(human, ws, TaskCreate(title="C", status="ready", depends_on=[b.key]))
    assert app.tasks.get(human, ws, c.key).status == "ready"
    app.tasks.claim(human, ws, c.key, ClaimRequest())
    app.tasks.complete(human, ws, c.key, summary="done")
    with pytest.raises(ConflictError):
        app.verification.accept_manually(human, ws, c.key, reason="too early")
    assert app.tasks.get(human, ws, c.key).status == "awaiting_review"


def test_cycle_detection(app, human, ws):
    a = app.tasks.create(human, ws, TaskCreate(title="A"))
    b = app.tasks.create(human, ws, TaskCreate(title="B", depends_on=[a.key]))
    c = app.tasks.create(human, ws, TaskCreate(title="C", depends_on=[b.key]))
    with pytest.raises(DependencyCycleError) as info:
        app.tasks.add_dependency(human, ws, a.key, c.key)
    assert "cycle" in info.value.message
    with pytest.raises(DependencyCycleError):
        app.tasks.add_dependency(human, ws, a.key, a.key)


def test_parent_child_rollup_and_hierarchy_cycle(app, human, ws):
    parent = app.tasks.create(human, ws, TaskCreate(title="Parent"))
    c1 = app.tasks.create(human, ws, TaskCreate(title="C1", parent=parent.key, status="ready"))
    c2 = app.tasks.create(human, ws, TaskCreate(title="C2", parent=parent.key))
    grandchild = app.tasks.create(human, ws, TaskCreate(title="GC", parent=c2.key))
    app.tasks.claim(human, ws, c1.key, ClaimRequest())
    app.tasks.complete(human, ws, c1.key)
    app.verification.accept_manually(human, ws, c1.key, reason="ok")
    rollup = app.tasks.rollup(human, ws, parent.key)
    assert rollup is not None
    assert rollup.total_descendants == 3
    assert rollup.accepted == 1
    assert rollup.progress == pytest.approx(1 / 3, abs=1e-3)
    with pytest.raises(DependencyCycleError):
        app.tasks.update(human, ws, parent.key, TaskUpdate(parent=grandchild.key))
    detail = app.tasks.detail(human, ws, parent.key)
    assert set(detail.child_ids) == {c1.id, c2.id}


def test_claims_are_exclusive(app, human, ws):
    t = app.tasks.create(human, ws, TaskCreate(title="T", status="ready"))
    from workgraph.core.context import RequestContext

    a1 = RequestContext.agent("one")
    a2 = RequestContext.agent("two")
    claimed = app.tasks.claim(a1, ws, t.key, ClaimRequest())
    assert claimed.status == "claimed"
    assert claimed.claimed_by == "agent:one"
    with pytest.raises(ResourceBusyError):
        app.tasks.claim(a2, ws, t.key, ClaimRequest())
    with pytest.raises(PermissionDenied):
        app.tasks.release(a2, ws, t.key)
    released = app.tasks.release(a1, ws, t.key)
    assert released.status == "ready"


def test_agent_cannot_accept(app, agent_ctx, human, ws):
    t = app.tasks.create(human, ws, TaskCreate(title="T", status="ready"))
    app.tasks.claim(agent_ctx, ws, t.key, ClaimRequest())
    done = app.tasks.complete(agent_ctx, ws, t.key, summary="all good")
    assert done.status == "awaiting_review"
    with pytest.raises(PermissionDenied):
        app.verification.accept_manually(agent_ctx, ws, t.key, reason="trust me")


def test_events_written_atomically_and_immutable(app, human, ws):
    t = app.tasks.create(human, ws, TaskCreate(title="T"))
    app.tasks.update(human, ws, t.key, TaskUpdate(priority="high", add_tags=["x"]))
    history = app.history.task_history(human, ws, t.key)
    types = [e.event_type for e in history]
    assert types[:2] == ["task.created", "task.updated"]
    assert all(e.correlation_id for e in history)
    with app.db.session(write=True) as session, pytest.raises(Exception, match="append-only"):
        session.execute(text("UPDATE events SET actor = 'x'"))
    with app.db.session(write=True) as session, pytest.raises(Exception, match="append-only"):
        session.execute(text("DELETE FROM events"))


def test_failed_mutation_writes_no_event(app, human, ws):
    t = app.tasks.create(human, ws, TaskCreate(title="T"))
    before = len(app.history.task_history(human, ws, t.key))
    with pytest.raises(NotFoundError):
        app.tasks.update(human, ws, t.key, TaskUpdate(title="changed", parent="WG-999"))
    assert app.tasks.get(human, ws, t.key).title == "T"
    assert len(app.history.task_history(human, ws, t.key)) == before


def test_projection_rebuild_matches_tables(app, human, ws):
    a = app.tasks.create(human, ws, TaskCreate(title="A", tags=["one"]))
    b = app.tasks.create(human, ws, TaskCreate(title="B", status="ready", parent=a.key))
    app.tasks.add_dependency(human, ws, b.key, a.key)
    app.tasks.update(
        human, ws, a.key, TaskUpdate(title="A2", remove_tags=["one"], add_tags=["two"], priority="low")
    )
    app.tasks.archive(human, ws, a.key)
    app.tasks.add_relation(human, ws, a.key, RelationCreate(target=b.key))
    report = app.history.verify_task_projection(human, ws)
    assert report["consistent"], report["mismatches"]
    assert report["tasks"] == 2


def test_filters_pagination_search_and_bulk(app, human, ws):
    for i in range(12):
        app.tasks.create(
            human,
            ws,
            TaskCreate(
                title=f"Task {i} {'database' if i % 3 == 0 else 'ui'}", priority="high" if i < 4 else "low"
            ),
        )
    page = app.tasks.list(human, ws, TaskFilter(limit=5))
    assert page.total == 12 and len(page.items) == 5 and page.next_offset == 5
    high = app.tasks.list(human, ws, TaskFilter(priorities=["high"], limit=100))
    assert high.total == 4
    found = app.tasks.list(human, ws, TaskFilter(text="datab", limit=100))
    assert found.total == 4
    result = app.tasks.bulk_update(
        human, ws, BulkUpdate(tasks=["WG-1", "WG-2", "WG-404"], update=TaskUpdate(add_tags=["bulk"]))
    )
    assert result.updated == ["WG-1", "WG-2"]
    assert "WG-404" in result.failed
    assert app.tasks.list(human, ws, TaskFilter(tags=["bulk"])).total == 2
    app.tasks.archive(human, ws, "WG-1")
    assert app.tasks.list(human, ws, TaskFilter(limit=100)).total == 11
    assert app.tasks.list(human, ws, TaskFilter(archived_only=True)).total == 1


def test_comments_are_searchable(app, human, ws):
    t = app.tasks.create(human, ws, TaskCreate(title="Plain"))
    app.tasks.add_comment(human, ws, t.key, CommentCreate(body="mentions kubernetes"))
    assert app.tasks.list(human, ws, TaskFilter(text="kubernetes")).total == 1


def test_multi_workspace_isolation(app, human, ws):
    app.workspaces.create(human, WorkspaceCreate(slug="other", name="Other", key_prefix="OT"))
    t = app.tasks.create(human, ws, TaskCreate(title="Main task"))
    o = app.tasks.create(human, "other", TaskCreate(title="Other task"))
    assert o.key == "OT-1"
    with pytest.raises(NotFoundError):
        app.tasks.get(human, "other", t.key)
    with pytest.raises(NotFoundError):
        app.tasks.add_dependency(human, ws, t.key, o.id)
    assert app.tasks.list(human, "other").total == 1


def test_graph(app, human, ws):
    a = app.tasks.create(human, ws, TaskCreate(title="A", status="ready"))
    b = app.tasks.create(
        human,
        ws,
        TaskCreate(title="B", status="ready", depends_on=[a.key], parent=a.key, tags=["area:x", "rho"]),
    )
    graph = app.tasks.graph(human, ws)
    assert {n.key: n.tags for n in graph.nodes} == {a.key: [], b.key: ["area:x", "rho"]}
    kinds = sorted(e.kind for e in graph.edges)
    assert kinds == ["depends_on", "parent"]
    blocked = {n.key: n.blocked for n in graph.nodes}
    assert blocked == {a.key: False, b.key: True}


def _accept_ready_task(app, human, ws, key):
    app.tasks.claim(human, ws, key, ClaimRequest())
    app.tasks.complete(human, ws, key, summary="done")
    app.verification.accept_manually(human, ws, key, reason="reviewed by hand")


def test_promote_ready_moves_backlog_whose_dependencies_are_accepted(app, human, ws):
    a = app.tasks.create(human, ws, TaskCreate(title="A", status="ready"))
    e = app.tasks.create(human, ws, TaskCreate(title="E", status="ready"))
    b = app.tasks.create(human, ws, TaskCreate(title="B", depends_on=[a.key]))
    f = app.tasks.create(human, ws, TaskCreate(title="F", depends_on=[a.key, e.key]))
    g = app.tasks.create(human, ws, TaskCreate(title="G"))
    _accept_ready_task(app, human, ws, a.key)

    promoted = app.tasks.promote_ready(human, ws)

    assert promoted == [b.key, g.key]
    assert app.tasks.get(human, ws, b.key).status == "ready"
    assert app.tasks.get(human, ws, g.key).status == "ready"
    # One unaccepted dependency keeps the task in backlog.
    assert app.tasks.get(human, ws, f.key).status == "backlog"
    assert app.tasks.get(human, ws, e.key).status == "ready"
    assert app.tasks.get(human, ws, a.key).status == "accepted"


def test_promote_ready_skips_parent_and_parked_tasks(app, human, ws):
    parent = app.tasks.create(human, ws, TaskCreate(title="Parent"))
    child = app.tasks.create(human, ws, TaskCreate(title="Child", parent=parent.key))
    parked = app.tasks.create(human, ws, TaskCreate(title="Parked", tags=["parked"]))
    plain = app.tasks.create(human, ws, TaskCreate(title="Plain"))

    promoted = app.tasks.promote_ready(human, ws)

    assert promoted == [child.key, plain.key]
    assert app.tasks.get(human, ws, parent.key).status == "backlog"
    assert app.tasks.get(human, ws, parked.key).status == "backlog"


def test_promote_ready_filters_by_tag(app, human, ws):
    sprint = app.tasks.create(human, ws, TaskCreate(title="Sprint", tags=["sprint-42"]))
    other = app.tasks.create(human, ws, TaskCreate(title="Other"))

    assert app.tasks.promote_ready(human, ws, tags=["sprint-42"]) == [sprint.key]
    assert app.tasks.get(human, ws, other.key).status == "backlog"


def test_promote_ready_dry_run_changes_nothing(app, human, ws):
    a = app.tasks.create(human, ws, TaskCreate(title="A", status="ready"))
    b = app.tasks.create(human, ws, TaskCreate(title="B", depends_on=[a.key]))
    _accept_ready_task(app, human, ws, a.key)
    before = app.tasks.get(human, ws, b.key)

    assert app.tasks.promote_ready(human, ws, dry_run=True) == [b.key]

    after = app.tasks.get(human, ws, b.key)
    assert after.status == "backlog"
    assert after.version == before.version
    assert app.tasks.promote_ready(human, ws) == [b.key]
    assert app.tasks.get(human, ws, b.key).status == "ready"


def test_promote_ready_report_counts_each_project(app, human, ws):
    app.projects.create(human, ws, ProjectCreate(slug="core", name="Core"))
    app.projects.create(human, ws, ProjectCreate(slug="ops", name="Ops"))
    both = app.tasks.create(human, ws, TaskCreate(title="Both", projects=["core", "ops"]))
    core_only = app.tasks.create(human, ws, TaskCreate(title="Core", projects=["core"]))
    unfiled = app.tasks.create(human, ws, TaskCreate(title="Unfiled"))

    report = app.tasks.promote_ready_report(human, ws, dry_run=True)

    assert report["promoted"] == [both.key, core_only.key, unfiled.key]
    assert report["by_project"] == {"(none)": 1, "core": 2, "ops": 1}
    assert app.tasks.get(human, ws, both.key).status == "backlog"

    report = app.tasks.promote_ready_report(human, ws, project="core")
    assert report == {"promoted": [both.key, core_only.key], "by_project": {"core": 2, "ops": 1}}
    assert app.tasks.get(human, ws, both.key).status == "ready"
    assert app.tasks.get(human, ws, unfiled.key).status == "backlog"


def test_accept_then_promote_dependents(app, human, ws):
    a = app.tasks.create(human, ws, TaskCreate(title="A", status="ready"))
    x = app.tasks.create(human, ws, TaskCreate(title="X", status="ready"))
    b = app.tasks.create(human, ws, TaskCreate(title="B", depends_on=[a.key]))
    c = app.tasks.create(human, ws, TaskCreate(title="C", depends_on=[a.key, x.key]))
    d = app.tasks.create(human, ws, TaskCreate(title="D", depends_on=[a.key], tags=["parked"]))
    _accept_ready_task(app, human, ws, a.key)

    assert app.tasks.promote_dependents(human, ws, a.key) == [b.key]
    assert app.tasks.get(human, ws, b.key).status == "ready"
    assert app.tasks.get(human, ws, c.key).status == "backlog"
    assert app.tasks.get(human, ws, d.key).status == "backlog"
