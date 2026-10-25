"""GV-19 golden vectors: task dependency, cycle, promotion, rollup and graph rules.

Drives a temporary Workgraph (``WORKGRAPH_HOME`` in a fresh temporary directory) through the public
task service with a fixed clock and sequential identifiers. Curated cases mirror the scenarios of
``tests/unit/test_tasks_core.py``; generated cases are random task graphs from a seed. Each case is
written to ``golden/taskgraph/<case>.json`` with its steps (input) and observations (output).
Identifiers become ``<task:KEY>`` or ``<uuid:N>`` and timestamps become ``<ts>``, so two runs give
identical bytes.

Usage: ``uv run python tools/golden/taskgraph.py``
"""

from __future__ import annotations

import json
import os
import random
import re
import shutil
import string
import sys
import tempfile
from collections.abc import Callable
from datetime import UTC, datetime, timedelta
from itertools import count
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
OUT_DIR = REPO / "golden" / "taskgraph"
FIXED_START = datetime(2026, 1, 1, tzinfo=UTC)
GENERATED_SEEDS = range(1, 13)

UUID_RE = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
TS_RE = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})")


def install_fixed_clock() -> None:
    """Make ``workgraph.core.clock`` return a fixed clock and sequential identifiers.

    This must run before any other Workgraph module is imported: the ORM binds ``utcnow`` and
    ``new_id`` as column defaults when its classes are defined, so only a patch made first is seen.
    ``workgraph.core.clock`` imports nothing from Workgraph, so importing it first is safe.
    """
    from workgraph.core import clock

    ticks = count()
    sequence = count(1)

    def utcnow() -> datetime:
        return FIXED_START + timedelta(seconds=next(ticks))

    def new_id() -> str:
        return f"00000000-0000-4000-8000-{next(sequence):012x}"

    clock.utcnow = utcnow
    clock.new_id = new_id
    assert "workgraph.app" not in sys.modules, "install_fixed_clock must run before workgraph.app loads"


class Normalizer:
    """Rewrites identifiers and timestamps in the document to stable placeholders."""

    def __init__(self, keys_by_id: dict[str, str]) -> None:
        self.keys_by_id = keys_by_id
        self.other: dict[str, str] = {}

    def _uuid(self, match: re.Match[str]) -> str:
        value = match.group(0)
        if value in self.keys_by_id:
            return f"<task:{self.keys_by_id[value]}>"
        if value not in self.other:
            self.other[value] = f"<uuid:{len(self.other) + 1}>"
        return self.other[value]

    def __call__(self, value: Any) -> Any:
        if isinstance(value, dict):
            return {k: self(v) for k, v in value.items()}
        if isinstance(value, list):
            return [self(v) for v in value]
        if isinstance(value, str):
            return TS_RE.sub("<ts>", UUID_RE.sub(self._uuid, value))
        return value


class Case:
    """One task graph in its own workspace. Every step and its outcome is recorded."""

    def __init__(self, app: Any, index: int, name: str, source: str, seed: int | None = None) -> None:
        from workgraph.core.context import RequestContext
        from workgraph.core.models.schemas import WorkspaceCreate

        self.app = app
        self.name = name
        self.source = source
        self.seed = seed
        self.human = RequestContext.human("gv19")
        self.slug = f"gv{index:02d}"
        letters = string.ascii_uppercase
        self.prefix = "T" + letters[index // 26] + letters[index % 26]
        self.workspace = app.workspaces.create(
            self.human, WorkspaceCreate(slug=self.slug, name=name, key_prefix=self.prefix)
        )
        self.keys_by_id: dict[str, str] = {}
        self.ids_by_key: dict[str, str] = {}
        self.steps: list[dict[str, Any]] = []

    # ------------------------------------------------------------------ steps

    def _step(self, op: str, args: dict[str, Any], action: Callable[[], Any]) -> Any:
        step: dict[str, Any] = {"op": op, **args}
        try:
            result = action()
        except Exception as exc:  # the error class and message are part of the vector
            step["outcome"] = {"error": type(exc).__name__, "message": str(exc)}
            self.steps.append(step)
            return None
        step["outcome"] = "ok"
        self.steps.append(step)
        return result

    def create(
        self,
        title: str,
        *,
        status: str | None = None,
        parent: str | None = None,
        depends_on: list[str] | tuple[str, ...] = (),
        tags: list[str] | tuple[str, ...] = (),
    ) -> str:
        from workgraph.core.models.schemas import TaskCreate

        kwargs: dict[str, Any] = {
            "title": title,
            "parent": parent,
            "depends_on": list(depends_on),
            "tags": list(tags),
        }
        if status is not None:
            kwargs["status"] = status
        args = {
            "title": title,
            "status": status,
            "parent": parent,
            "depends_on": list(depends_on),
            "tags": list(tags),
        }
        out = self._step(
            "create", args, lambda: self.app.tasks.create(self.human, self.slug, TaskCreate(**kwargs))
        )
        if out is None:
            raise RuntimeError(f"{self.name}: create {title!r} failed: {self.steps[-1]['outcome']}")
        self.keys_by_id[out.id] = out.key
        self.ids_by_key[out.key] = out.id
        return out.key

    def claim(self, key: str) -> None:
        from workgraph.core.models.schemas import ClaimRequest

        self._step(
            "claim", {"task": key}, lambda: self.app.tasks.claim(self.human, self.slug, key, ClaimRequest())
        )

    def complete(self, key: str) -> None:
        self._step(
            "complete",
            {"task": key, "summary": "done"},
            lambda: self.app.tasks.complete(self.human, self.slug, key, summary="done"),
        )

    def accept(self, key: str) -> None:
        self._step(
            "accept",
            {"task": key},
            lambda: self.app.verification.accept_manually(
                self.human, self.slug, key, reason="reviewed by hand"
            ),
        )

    def claim_and_accept(self, key: str) -> None:
        self.claim(key)
        self.complete(key)
        self.accept(key)

    def set_status(self, key: str, status: str) -> None:
        from workgraph.core.models.schemas import TaskUpdate

        self._step(
            "set_status",
            {"task": key, "status": status},
            lambda: self.app.tasks.update(self.human, self.slug, key, TaskUpdate(status=status)),
        )

    def archive(self, key: str) -> None:
        self._step("archive", {"task": key}, lambda: self.app.tasks.archive(self.human, self.slug, key))

    def add_dependency(self, task: str, depends_on: str) -> None:
        self._step(
            "add_dependency",
            {"task": task, "depends_on": depends_on},
            lambda: self.app.tasks.add_dependency(self.human, self.slug, task, depends_on),
        )

    def set_parent(self, task: str, parent: str) -> None:
        from workgraph.core.models.schemas import TaskUpdate

        self._step(
            "set_parent",
            {"task": task, "parent": parent},
            lambda: self.app.tasks.update(self.human, self.slug, task, TaskUpdate(parent=parent)),
        )

    def add_stack(self, parent: str, status: str, items: list[tuple[str, str]]) -> None:
        """Insert a stack for ``parent`` with one item per ``(child, item state)``."""
        from workgraph.persistence.orm.stacks import Stack, StackItem

        def insert() -> None:
            with self.app.db.session(write=True) as session:
                stack = Stack(
                    workspace_id=self.workspace.id,
                    parent_task_id=self.ids_by_key[parent],
                    status=status,
                )
                session.add(stack)
                session.flush()
                for child, state in items:
                    session.add(StackItem(stack_id=stack.id, task_id=self.ids_by_key[child], state=state))
                session.flush()

        self._step(
            "add_stack",
            {"parent": parent, "status": status, "items": [list(item) for item in items]},
            insert,
        )

    def promote(self, *, dry_run: bool) -> list[str] | None:
        return self._step(
            "promote_ready",
            {"dry_run": dry_run},
            lambda: self.app.tasks.promote_ready(self.human, self.slug, dry_run=dry_run),
        )

    # ----------------------------------------------------------- observations

    def observe(self) -> dict[str, Any]:
        from sqlalchemy import select

        from workgraph.core.services.tasks import (
            depends_on_reachable,
            is_ancestor,
            satisfied_dependencies,
            unsatisfied_dependencies,
        )
        from workgraph.persistence.orm.core import Task, TaskDependency, TaskTag

        key_of = self.keys_by_id
        keys = sorted(self.ids_by_key, key=lambda k: int(k.rsplit("-", 1)[1]))
        with self.app.db.session() as session:
            task_ids = list(self.ids_by_key.values())
            task_rows = {
                row.id: row
                for row in session.execute(
                    select(Task.id, Task.status, Task.parent_id, Task.archived_at).where(
                        Task.id.in_(task_ids)
                    )
                )
            }
            tags: dict[str, list[str]] = {tid: [] for tid in task_ids}
            for tid, tag in session.execute(
                select(TaskTag.task_id, TaskTag.tag).where(TaskTag.task_id.in_(task_ids))
            ):
                tags[tid].append(tag)
            dep_pairs = sorted(
                (key_of[t], key_of[d])
                for t, d in session.execute(
                    select(TaskDependency.task_id, TaskDependency.depends_on_id).where(
                        TaskDependency.task_id.in_(task_ids)
                    )
                )
            )
            edge_ids = [(self.ids_by_key[t], self.ids_by_key[d]) for t, d in dep_pairs]
            satisfied = satisfied_dependencies(session, edge_ids)
            unsatisfied = {
                key: [b.key for b in unsatisfied_dependencies(session, self.ids_by_key[key])] for key in keys
            }
            reachable: dict[str, list[str] | None] = {}
            ancestor: dict[str, bool] = {}
            for a in keys:
                for b in keys:
                    path = depends_on_reachable(session, self.ids_by_key[a], self.ids_by_key[b])
                    reachable[f"{a}>{b}"] = None if path is None else [key_of[p] for p in path]
                    ancestor[f"{a}>{b}"] = is_ancestor(session, self.ids_by_key[a], self.ids_by_key[b])
            statuses = {
                key_of[tid]: {
                    "status": row.status,
                    "parent": key_of[row.parent_id] if row.parent_id else None,
                    "archived": row.archived_at is not None,
                    "tags": sorted(tags[tid]),
                }
                for tid, row in task_rows.items()
            }

        ready = [t.key for t in self.app.tasks.get_ready(self.human, self.slug, limit=500)]
        rollups: dict[str, Any] = {}
        for key in keys:
            rollup = self.app.tasks.rollup(self.human, self.slug, key)
            rollups[key] = None if rollup is None else rollup.model_dump(mode="json")

        graphs = {
            "all": self.app.tasks.graph(self.human, self.slug),
            "open_only": self.app.tasks.graph(self.human, self.slug, include_closed=False),
        }
        with_children = [k for k in keys if rollups[k] is not None]
        if with_children:
            graphs["root_first_parent"] = self.app.tasks.graph(self.human, self.slug, root=with_children[0])
        graph_out: dict[str, Any] = {}
        for label, graph in graphs.items():
            dumped = graph.model_dump(mode="json")
            # Edge order is a storage detail; the vector fixes it by (kind, source key, target key, relation).
            dumped["edges"] = sorted(
                dumped["edges"],
                key=lambda e: (
                    e["kind"],
                    key_of.get(e["source"], e["source"]),
                    key_of.get(e["target"], e["target"]),
                    e.get("relation_type") or "",
                ),
            )
            graph_out[label] = dumped

        return {
            "tasks": statuses,
            "dependency_edges": [list(pair) for pair in dep_pairs],
            "satisfied": [
                [t, d, (self.ids_by_key[t], self.ids_by_key[d]) in satisfied] for t, d in dep_pairs
            ],
            "unsatisfied_blockers": unsatisfied,
            "reachable_path": reachable,
            "is_ancestor": ancestor,
            "ready": ready,
            "rollup": rollups,
            "graph": graph_out,
        }

    def finish(self, observations: dict[str, Any]) -> dict[str, Any]:
        document = {
            "case": self.name,
            "source": self.source,
            "seed": self.seed,
            "steps": self.steps,
            "observations": observations,
        }
        return Normalizer(self.keys_by_id)(document)


# ------------------------------------------------------------------ curated cases


def curated_dependencies_block_and_unblock(case: Case) -> dict[str, Any]:
    a = case.create("A", status="ready")
    case.create("B", status="ready", depends_on=[a])
    snapshots = {"before_completion": case.observe()}
    case.claim(a)
    case.complete(a)
    snapshots["after_completion"] = case.observe()
    case.accept(a)
    snapshots["after_acceptance"] = case.observe()
    return snapshots


def curated_cycle_detection(case: Case) -> dict[str, Any]:
    a = case.create("A")
    b = case.create("B", depends_on=[a])
    c = case.create("C", depends_on=[b])
    case.add_dependency(a, c)
    case.add_dependency(a, a)
    return {"observed": case.observe()}


def curated_parent_rollup_and_hierarchy_cycle(case: Case) -> dict[str, Any]:
    parent = case.create("Parent")
    c1 = case.create("C1", status="ready", parent=parent)
    c2 = case.create("C2", parent=parent)
    grandchild = case.create("GC", parent=c2)
    case.claim_and_accept(c1)
    case.set_parent(parent, grandchild)
    return {"observed": case.observe()}


def curated_promote_moves_accepted_dependencies(case: Case) -> dict[str, Any]:
    a = case.create("A", status="ready")
    e = case.create("E", status="ready")
    case.create("B", depends_on=[a])
    case.create("F", depends_on=[a, e])
    case.create("G")
    case.claim_and_accept(a)
    before = case.observe()
    dry = case.promote(dry_run=True)
    promoted = case.promote(dry_run=False)
    return {"before": before, "dry_run": dry, "promoted": promoted, "after": case.observe()}


def curated_promote_skips_parent_and_parked(case: Case) -> dict[str, Any]:
    parent = case.create("Parent")
    case.create("Child", parent=parent)
    case.create("Parked", tags=["parked"])
    case.create("Plain")
    dry = case.promote(dry_run=True)
    promoted = case.promote(dry_run=False)
    return {"dry_run": dry, "promoted": promoted, "after": case.observe()}


def curated_graph_basic(case: Case) -> dict[str, Any]:
    a = case.create("A", status="ready")
    case.create("B", status="ready", depends_on=[a], parent=a, tags=["area:x", "rho"])
    return {"observed": case.observe()}


def curated_stack_sibling_states(case: Case) -> dict[str, Any]:
    """A sibling dependency is satisfied only inside an active stack, once its item is ready or later."""
    variants = [
        ("pending_item_open_stack", "open", "pending"),
        ("ready_item_open_stack", "open", "ready"),
        ("verified_item_verifying_stack", "verifying", "verified"),
        ("merged_item_open_stack", "open", "merged"),
        ("conflict_item_open_stack", "open", "conflict"),
        ("ready_item_landed_stack", "landed", "ready"),
        ("ready_item_abandoned_stack", "abandoned", "ready"),
    ]
    for label, stack_status, item_state in variants:
        parent = case.create(f"{label} parent")
        first = case.create(f"{label} first", parent=parent, status="ready")
        second = case.create(f"{label} second", parent=parent, depends_on=[first])
        case.add_stack(parent, stack_status, [(first, item_state), (second, "pending")])
    return {"observed": case.observe()}


CURATED: list[tuple[str, Callable[[Case], dict[str, Any]], str]] = [
    (
        "dependencies_block_and_unblock",
        curated_dependencies_block_and_unblock,
        "tests/unit/test_tasks_core.py::test_dependencies_block_and_unblock",
    ),
    ("cycle_detection", curated_cycle_detection, "tests/unit/test_tasks_core.py::test_cycle_detection"),
    (
        "parent_rollup_and_hierarchy_cycle",
        curated_parent_rollup_and_hierarchy_cycle,
        "tests/unit/test_tasks_core.py::test_parent_child_rollup_and_hierarchy_cycle",
    ),
    (
        "promote_moves_accepted_dependencies",
        curated_promote_moves_accepted_dependencies,
        "tests/unit/test_tasks_core.py::test_promote_ready_moves_backlog_whose_dependencies_are_accepted",
    ),
    (
        "promote_skips_parent_and_parked",
        curated_promote_skips_parent_and_parked,
        "tests/unit/test_tasks_core.py::test_promote_ready_skips_parent_and_parked_tasks",
    ),
    ("graph_basic", curated_graph_basic, "tests/unit/test_tasks_core.py::test_graph"),
    (
        "stack_sibling_states",
        curated_stack_sibling_states,
        "no Python unit test; the stack rule of satisfied_dependencies in core/services/tasks.py",
    ),
]


# ------------------------------------------------------------------ generated cases


def build_generated(case: Case, seed: int) -> dict[str, Any]:
    """A random task graph: parents, tags, dependencies, lifecycle moves, stacks, cycle attempts."""
    rng = random.Random(seed)  # noqa: S311 - seeded test-data generator, not a security boundary
    size = rng.randint(6, 14)
    keys: list[str] = []
    ready_keys: set[str] = set()
    parent_of: dict[str, str | None] = {}
    for i in range(size):
        parent = rng.choice(keys) if keys and rng.random() < 0.25 else None
        deps = rng.sample(keys, min(rng.choice([0, 0, 1, 1, 2, 3]), len(keys)))
        tags = [t for t in ("parked", "area:x", "rho") if rng.random() < 0.2]
        status = rng.choice(["backlog", "ready", "ready"])
        key = case.create(f"Task {i}", status=status, parent=parent, depends_on=deps, tags=tags)
        keys.append(key)
        parent_of[key] = parent
        if status == "ready":
            ready_keys.add(key)

    for key in keys:
        roll = rng.random()
        if roll < 0.35 and key in ready_keys:
            case.claim_and_accept(key)
        elif roll < 0.35:
            case.claim(key)  # a backlog task cannot be claimed: the refusal is part of the vector
        elif roll < 0.42:
            case.set_status(key, "cancelled")
        elif roll < 0.48:
            case.archive(key)

    for parent in sorted({p for p in parent_of.values() if p is not None}):
        if rng.random() < 0.5:
            continue
        children = [k for k in keys if parent_of[k] == parent]
        states = ["pending", "ready", "merged", "conflict", "verified", "ejected"]
        items = [(child, rng.choice(states)) for child in children]
        status = rng.choice(["open", "open", "verifying", "landed", "abandoned"])
        case.add_stack(parent, status, items)

    for _ in range(8):
        a, b = rng.sample(keys, 2)
        case.add_dependency(a, b)
    for _ in range(4):
        a, b = rng.sample(keys, 2)
        case.set_parent(a, b)
    case.add_dependency(keys[0], keys[0])

    observations: dict[str, Any] = {"before_promotion": case.observe()}
    observations["dry_run"] = case.promote(dry_run=True)
    observations["promoted"] = case.promote(dry_run=False)
    observations["after_promotion"] = case.observe()
    return observations


def write_document(document: dict[str, Any]) -> None:
    text = json.dumps(document, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    (OUT_DIR / f"{document['case']}.json").write_text(text, encoding="utf-8", newline="\n")


def main() -> None:
    home = Path(tempfile.mkdtemp(prefix="wg-gv19-"))
    previous_home = os.environ.get("WORKGRAPH_HOME")
    os.environ["WORKGRAPH_HOME"] = str(home)
    try:
        install_fixed_clock()
        from workgraph.app import Workgraph
        from workgraph.config.paths import AppPaths

        app = Workgraph(AppPaths.resolve(home))
        config = app.config
        config.llm.auto_classify = False
        config.llm.auto_embed = False
        app.config_store.save_global(config)

        OUT_DIR.mkdir(parents=True, exist_ok=True)
        for stale in OUT_DIR.glob("*.json"):
            stale.unlink()

        index = 0
        for name, fn, source in CURATED:
            case = Case(app, index, name, source)
            index += 1
            write_document(case.finish(fn(case)))

        for seed in GENERATED_SEEDS:
            case = Case(app, index, f"generated_{seed:02d}", "generated: random task graph", seed=seed)
            index += 1
            write_document(case.finish(build_generated(case, seed)))

        app.close()
        print(f"wrote {index} cases to {OUT_DIR}")
    finally:
        if previous_home is None:
            os.environ.pop("WORKGRAPH_HOME", None)
        else:
            os.environ["WORKGRAPH_HOME"] = previous_home
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
