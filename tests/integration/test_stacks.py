from __future__ import annotations

import pytest
from sqlalchemy import select

from workgraph.core.context import RequestContext
from workgraph.core.errors import (
    ConflictError,
    InvalidTransitionError,
    NotFoundError,
    PermissionDenied,
    ValidationFailed,
)
from workgraph.core.models.enums import StackItemState
from workgraph.core.models.schemas import RepositoryRegister, TaskCreate
from workgraph.persistence.orm import Event, Stack
from workgraph.security.permissions import DEFAULT_AGENT_PERMISSIONS, Permission


@pytest.fixture
def tree(app, human, ws, make_repo):
    """A repository and a parent task with two children linked to it."""
    repo = make_repo("svc")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    parent = app.tasks.create(human, ws, TaskCreate(title="Parent work", repositories=["svc"]))
    first = app.tasks.create(
        human, ws, TaskCreate(title="First change", parent=parent.key, repositories=["svc"])
    )
    second = app.tasks.create(
        human, ws, TaskCreate(title="Second change", parent=parent.key, repositories=["svc"])
    )
    return repo, parent, first, second


def _events(app, entity_id: str) -> list[str]:
    with app.db.session() as session:
        return list(
            session.scalars(select(Event.event_type).where(Event.entity_id == entity_id).order_by(Event.seq))
        )


def _state(stack, key: str) -> str:
    return next(item.state for item in stack.items if item.task_key == key)


def test_create_stack_records_base_sha_branch_and_pending_items(app, human, ws, tree):
    repo, parent, first, second = tree
    stack = app.stacks.create(human, ws, parent.key)

    assert stack.parent_key == parent.key
    assert stack.status == "open"
    assert [(r.repository_name, r.base_sha, r.integration_branch) for r in stack.repositories] == [
        ("svc", repo.head(), f"wg/stack/{parent.key.lower()}")
    ]
    assert [(i.task_key, i.state, i.merge_order) for i in stack.items] == [
        (first.key, "pending", None),
        (second.key, "pending", None),
    ]
    assert _events(app, stack.id) == ["stack.created"]


def test_base_sha_is_fixed_at_creation(app, human, ws, tree):
    repo, parent, _, _ = tree
    base = repo.head()
    stack = app.stacks.create(human, ws, parent.key)
    repo.commit("move main on", {"NEW.md": "later\n"})

    assert stack.repositories[0].base_sha == base
    assert app.stacks.get(human, ws, parent.key).repositories[0].base_sha == base
    assert repo.head() != base


def test_item_transitions_follow_the_table(app, human, ws, tree):
    _, parent, first, _ = tree
    app.stacks.create(human, ws, parent.key)

    stack = app.stacks.set_item_state(human, ws, parent.key, first.key, StackItemState.READY)
    assert _state(stack, first.key) == "ready"
    stack = app.stacks.set_item_state(human, ws, parent.key, first.key, StackItemState.MERGED)
    assert _state(stack, first.key) == "merged"
    assert next(i.merge_order for i in stack.items if i.task_key == first.key) == 1
    stack = app.stacks.set_item_state(
        human, ws, parent.key, first.key, StackItemState.CONFLICT, detail="src/a.py conflicts"
    )
    assert next(i for i in stack.items if i.task_key == first.key).detail == "src/a.py conflicts"

    with pytest.raises(InvalidTransitionError):
        app.stacks.set_item_state(human, ws, parent.key, first.key, StackItemState.VERIFIED)

    stack = app.stacks.set_item_state(human, ws, parent.key, first.key, StackItemState.READY)
    stack = app.stacks.set_item_state(human, ws, parent.key, first.key, StackItemState.MERGED)
    # Merge order is recorded the first time an item merges and survives later re-merges.
    assert next(i.merge_order for i in stack.items if i.task_key == first.key) == 1

    # Unchanged state is a no-op and records nothing.
    before = _events(app, app.stacks.get(human, ws, parent.key).id)
    app.stacks.set_item_state(human, ws, parent.key, first.key, StackItemState.MERGED)
    assert _events(app, app.stacks.get(human, ws, parent.key).id) == before


def test_add_and_remove_children(app, human, ws, tree):
    _, parent, first, second = tree
    stack = app.stacks.create(human, ws, parent.key)
    app.stacks.set_item_state(human, ws, parent.key, first.key, StackItemState.READY)
    app.stacks.set_item_state(human, ws, parent.key, first.key, StackItemState.MERGED)

    with pytest.raises(ConflictError):
        app.stacks.remove_item(human, ws, parent.key, first.key)

    stack = app.stacks.remove_item(human, ws, parent.key, second.key)
    assert [i.task_key for i in stack.items] == [first.key]
    assert _events(app, stack.id)[-1] == "stack.item_removed"

    late = app.tasks.create(
        human, ws, TaskCreate(title="Late change", parent=parent.key, repositories=["svc"])
    )
    stack = app.stacks.add_item(human, ws, parent.key, late.key)
    assert _state(stack, late.key) == "pending"
    assert _events(app, stack.id)[-1] == "stack.item_added"

    with pytest.raises(ConflictError):
        app.stacks.add_item(human, ws, parent.key, late.key)
    unrelated = app.tasks.create(human, ws, TaskCreate(title="Unrelated"))
    with pytest.raises(ValidationFailed):
        app.stacks.add_item(human, ws, parent.key, unrelated.key)
    with pytest.raises(NotFoundError):
        app.stacks.remove_item(human, ws, parent.key, unrelated.key)


def test_readmit_requeues_an_ejected_or_pending_child(app, human, ws, tree):
    _, parent, first, second = tree
    app.stacks.create(human, ws, parent.key)
    for state in (StackItemState.READY, StackItemState.MERGED, StackItemState.EJECTED):
        app.stacks.set_item_state(human, ws, parent.key, first.key, state)
    stack = app.stacks.readmit_item(human, ws, parent.key, first.key)
    assert _state(stack, first.key) == "ready"
    # A child added (or re-added) to the stack starts pending; readmit skips the new fast gate.
    stack = app.stacks.readmit_item(human, ws, parent.key, second.key)
    assert _state(stack, second.key) == "ready"
    app.stacks.set_item_state(human, ws, parent.key, first.key, StackItemState.MERGED)
    with pytest.raises(ConflictError):
        app.stacks.readmit_item(human, ws, parent.key, first.key)


def test_child_in_another_repository_is_rejected(app, human, ws, tree, make_repo):
    _, parent, _, _ = tree
    app.stacks.create(human, ws, parent.key)
    other = make_repo("other")
    app.repositories.register(human, ws, RepositoryRegister(path=str(other.path)))
    stray = app.tasks.create(
        human, ws, TaskCreate(title="Elsewhere", parent=parent.key, repositories=["other"])
    )
    with pytest.raises(ValidationFailed) as excinfo:
        app.stacks.add_item(human, ws, parent.key, stray.key)
    assert excinfo.value.details["repositories"] == ["other"]


def test_create_rejections(app, human, ws, tree):
    _, parent, _, _ = tree
    lonely = app.tasks.create(human, ws, TaskCreate(title="No children", repositories=["svc"]))
    with pytest.raises(ValidationFailed):
        app.stacks.create(human, ws, lonely.key)

    unlinked = app.tasks.create(human, ws, TaskCreate(title="Unlinked parent"))
    app.tasks.create(human, ws, TaskCreate(title="Orphan child", parent=unlinked.key))
    with pytest.raises(ValidationFailed):
        app.stacks.create(human, ws, unlinked.key)

    app.stacks.create(human, ws, parent.key)
    with pytest.raises(ConflictError):
        app.stacks.create(human, ws, parent.key)


def test_new_stack_allowed_once_previous_is_inactive(app, human, ws, tree):
    _, parent, _, _ = tree
    first = app.stacks.create(human, ws, parent.key)
    with app.db.session(write=True) as session:
        session.get(Stack, first.id).status = "abandoned"
    second = app.stacks.create(human, ws, parent.key)
    assert second.id != first.id
    assert app.stacks.get(human, ws, parent.key).id == second.id
    assert [s.id for s in app.stacks.list(human, ws)] == [second.id]
    assert {s.id for s in app.stacks.list(human, ws, include_inactive=True)} == {first.id, second.id}


def test_reads_and_for_task(app, human, ws, tree):
    _, parent, first, _ = tree
    stack = app.stacks.create(human, ws, parent.key)
    listed = app.stacks.list(human, ws)
    assert [s.id for s in listed] == [stack.id]
    assert app.stacks.get(human, ws, parent.key).id == stack.id
    [item] = app.stacks.for_task(human, ws, first.key)
    assert (item.task_key, item.state, item.stack_id) == (first.key, "pending", stack.id)
    assert app.stacks.for_task(human, ws, parent.key) == []
    with pytest.raises(NotFoundError):
        app.stacks.get(human, ws, "WG-999")


def test_agents_can_read_but_not_mutate(app, human, ws, tree):
    _, parent, first, _ = tree
    agent = RequestContext.agent("worker")
    assert Permission.STACK_WRITE not in DEFAULT_AGENT_PERMISSIONS

    with pytest.raises(PermissionDenied):
        app.stacks.create(agent, ws, parent.key)
    stack = app.stacks.create(human, ws, parent.key)
    with pytest.raises(PermissionDenied):
        app.stacks.set_item_state(agent, ws, parent.key, first.key, StackItemState.READY)
    with pytest.raises(PermissionDenied):
        app.stacks.add_item(agent, ws, parent.key, first.key)
    with pytest.raises(PermissionDenied):
        app.stacks.remove_item(agent, ws, parent.key, first.key)

    assert app.stacks.get(agent, ws, parent.key).id == stack.id
    assert app.stacks.list(agent, ws)[0].id == stack.id
    with pytest.raises(PermissionDenied):
        app.stacks.get(RequestContext.agent("blind", frozenset()), ws, parent.key)
