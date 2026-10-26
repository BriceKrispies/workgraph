"""Golden vectors for rewrite reconciliation, cherry-pick linking, cumulative churn and the final
change surface (the second half of CORE-10).

Three of the four vector sets run the real methods of ``GitService`` in
``integrations/git/service.py`` (``_detect_equivalences``, ``_carry_links``, ``_cumulative_churn``)
against an in-memory SQLite database built from the ORM models, so the expected output is what the
Python service decides. The method bodies take ``self`` only for ``self.db`` and sibling helpers,
so they are called with a stand-in object that provides exactly those.

``compute_change_surface`` is entangled with the git subprocess, the manifest tables and the
structural analyser, so its decision and record are replicated here line for line, with the git
questions answered from the case data. The replica is marked as such in the output.

Every case is generated from a fixed seed plus a list of hand-written cases; two runs write
identical bytes. Run from the repository root with
``uv run python tools/golden/gen_gitrewrite.py``. Writes ``golden/gitrewrite/*.json``.
"""

from __future__ import annotations

import json
import random
from contextlib import contextmanager
from datetime import datetime, timedelta
from pathlib import Path
from types import SimpleNamespace
from typing import Any

from sqlalchemy import create_engine, select
from sqlalchemy.orm import sessionmaker
from sqlalchemy.pool import StaticPool

from workgraph.analytics.code.classify import file_class
from workgraph.integrations.git.service import EMPTY_TREE, GitService
from workgraph.persistence.orm.base import Base
from workgraph.persistence.orm.git import (
    Commit,
    CommitAnalysis,
    CommitEquivalence,
    CommitFileChange,
    CommitTaskLink,
)

ROOT = Path(__file__).resolve().parents[2]
OUT_DIR = ROOT / "golden" / "gitrewrite"
SEED = 20251026
EPOCH = datetime(1970, 1, 1)  # noqa: DTZ001 - naive, like the DateTime columns it fills
RANDOM_EQUIVALENCE_CASES = 120
RANDOM_CARRY_CASES = 40
RANDOM_CHURN_CASES = 120
RANDOM_SURFACE_CASES = 120


def when(seconds: int | None) -> datetime | None:
    return None if seconds is None else EPOCH + timedelta(seconds=seconds)


def fresh_session_factory() -> sessionmaker:
    engine = create_engine("sqlite://", poolclass=StaticPool, connect_args={"check_same_thread": False})
    Base.metadata.create_all(engine)
    return sessionmaker(bind=engine, expire_on_commit=False)


class FakeDb:
    """The ``self.db`` of the service: ``session(write=...)`` yields a session in its own transaction."""

    def __init__(self, factory: sessionmaker) -> None:
        self._factory = factory

    @contextmanager
    def session(self, *, write: bool = False):
        session = self._factory()
        try:
            yield session
            if write:
                session.commit()
        except BaseException:
            session.rollback()
            raise
        finally:
            session.close()


def service_stub(factory: sessionmaker) -> SimpleNamespace:
    return SimpleNamespace(
        db=FakeDb(factory),
        _carry_links=GitService._carry_links,
    )


def sha_of(number: int) -> str:
    return f"{number:040x}"


# ---------------------------------------------------------------- equivalences


def equivalence_case(
    *,
    name: str,
    origin: str,
    repository: str,
    commits: list[dict[str, Any]],
    new_ids: list[str],
    existing: list[list[str]],
) -> dict[str, Any]:
    factory = fresh_session_factory()
    with factory() as session:
        for c in commits:
            session.add(
                Commit(
                    id=c["id"],
                    repository_id=c["repository_id"],
                    sha=c["sha"],
                    patch_id=c["patch_id"],
                    reachable=c["reachable"],
                    author_email=c["author_email"],
                    authored_at=when(c["authored_at"]),
                    subject=c["subject"],
                    message=c["message"],
                )
            )
        session.flush()
        for index, (orig, equiv) in enumerate(existing):
            session.add(
                CommitEquivalence(
                    id=f"pre-{index}",
                    repository_id=repository,
                    original_commit_id=orig,
                    equivalent_commit_id=equiv,
                    kind="rewrite",
                    evidence={},
                )
            )
        session.commit()
    stub = service_stub(factory)
    info = SimpleNamespace(id=repository)
    GitService._detect_equivalences(stub, info, new_ids)

    with factory() as session:
        rows = session.scalars(
            select(CommitEquivalence).where(
                CommitEquivalence.id.notin_([f"pre-{i}" for i in range(len(existing))])
            )
        ).all()
        produced = [
            {
                "original_id": row.original_commit_id,
                "equivalent_id": row.equivalent_commit_id,
                "kind": row.kind,
                "evidence": row.evidence,
            }
            for row in rows
        ]
    produced.sort(key=lambda r: (r["equivalent_id"], r["original_id"]))
    return {
        "id": name,
        "origin": origin,
        "input": {
            "repository_id": repository,
            "commits": commits,
            "new_ids": new_ids,
            "existing": existing,
        },
        "output": produced,
    }


def commit_row(
    cid: str,
    sha: str,
    repository: str = "r1",
    *,
    patch: str | None = None,
    reachable: bool = True,
    author: str = "a@example.com",
    authored: int | None = 1_700_000_000,
    subject: str = "change",
    message: str | None = None,
) -> dict[str, Any]:
    return {
        "id": cid,
        "sha": sha,
        "repository_id": repository,
        "patch_id": patch,
        "reachable": reachable,
        "author_email": author,
        "authored_at": authored,
        "subject": subject,
        "message": message if message is not None else subject,
    }


def hand_equivalence_cases() -> list[dict[str, Any]]:
    a, b, c = sha_of(1), sha_of(2), sha_of(3)
    cases = [
        equivalence_case(
            name="rebase_same_patch_unreachable_old",
            origin="hand-written: patch-id twin left every ref",
            repository="r1",
            commits=[
                commit_row("old", a, patch="p1", reachable=False),
                commit_row("new", b, patch="p1"),
            ],
            new_ids=["new"],
            existing=[],
        ),
        equivalence_case(
            name="cherry_same_patch_reachable_old",
            origin="hand-written: patch-id twin still reachable",
            repository="r1",
            commits=[commit_row("old", a, patch="p1"), commit_row("new", b, patch="p1")],
            new_ids=["new"],
            existing=[],
        ),
        equivalence_case(
            name="patch_twin_in_new_set_reachable_skipped",
            origin="hand-written: a reachable twin that is itself new is skipped",
            repository="r1",
            commits=[commit_row("x", a, patch="p1"), commit_row("y", b, patch="p1")],
            new_ids=["x", "y"],
            existing=[],
        ),
        equivalence_case(
            name="patch_twin_in_new_set_unreachable_matched",
            origin="hand-written: an unreachable twin in the new set still matches",
            repository="r1",
            commits=[commit_row("x", a, patch="p1", reachable=False), commit_row("y", b, patch="p1")],
            new_ids=["x", "y"],
            existing=[],
        ),
        equivalence_case(
            name="amend_author_date_subject",
            origin="hand-written: amend without a patch id",
            repository="r1",
            commits=[
                commit_row("old", a, reachable=False, subject="fix it"),
                commit_row("new", b, subject="fix it"),
            ],
            new_ids=["new"],
            existing=[],
        ),
        equivalence_case(
            name="amend_reachable_old_no_match",
            origin="hand-written: same author/date/subject but the old commit is reachable",
            repository="r1",
            commits=[commit_row("old", a, subject="fix it"), commit_row("new", b, subject="fix it")],
            new_ids=["new"],
            existing=[],
        ),
        equivalence_case(
            name="amend_skipped_when_patch_matched",
            origin="hand-written: the amend rule runs only when no patch twin matched",
            repository="r1",
            commits=[
                commit_row("old", a, patch="p1", reachable=False, subject="fix it"),
                commit_row("older", sha_of(9), reachable=False, subject="fix it"),
                commit_row("new", b, patch="p1", subject="fix it"),
            ],
            new_ids=["new"],
            existing=[],
        ),
        equivalence_case(
            name="cherry_pick_trailer",
            origin="hand-written: '(cherry picked from commit X)' names an older commit",
            repository="r1",
            commits=[
                commit_row("picked", a, message="base"),
                commit_row(
                    "new",
                    b,
                    message=f"pick it\n\n(cherry picked from commit {a[:10]})",
                ),
            ],
            new_ids=["new"],
            existing=[],
        ),
        equivalence_case(
            name="cherry_pick_trailer_not_duplicated",
            origin="hand-written: the trailer's commit is already a patch-id match",
            repository="r1",
            commits=[
                commit_row("picked", a, patch="p1"),
                commit_row("new", b, patch="p1", message=f"pick\n\n(cherry picked from commit {a[:7]})"),
            ],
            new_ids=["new"],
            existing=[],
        ),
        equivalence_case(
            name="cherry_pick_trailer_unknown_sha",
            origin="hand-written: a trailer naming no known commit creates nothing",
            repository="r1",
            commits=[commit_row("new", b, message=f"pick\n\n(cherry picked from commit {c[:12]})")],
            new_ids=["new"],
            existing=[],
        ),
        equivalence_case(
            name="other_repository_same_patch_ignored",
            origin="hand-written: identical patches in different repositories are distinct",
            repository="r1",
            commits=[
                commit_row("other", a, repository="r2", patch="p1", reachable=False),
                commit_row("new", b, patch="p1"),
            ],
            new_ids=["new"],
            existing=[],
        ),
        equivalence_case(
            name="existing_pair_not_repeated",
            origin="hand-written: a recorded pair is not written again",
            repository="r1",
            commits=[commit_row("old", a, patch="p1", reachable=False), commit_row("new", b, patch="p1")],
            new_ids=["new"],
            existing=[["old", "new"]],
        ),
        equivalence_case(
            name="no_new_commits",
            origin="hand-written: empty run",
            repository="r1",
            commits=[commit_row("old", a, patch="p1", reachable=False)],
            new_ids=[],
            existing=[],
        ),
    ]
    return cases


def random_equivalence_cases(rng: random.Random) -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    for index in range(RANDOM_EQUIVALENCE_CASES):
        size = rng.randint(1, 6)
        commits: list[dict[str, Any]] = []
        for n in range(size):
            message = rng.choice(["plain", "tidy up", "(cherry picked from commit {ref})"])
            commits.append(
                commit_row(
                    f"c{n}",
                    sha_of(index * 100 + n + 1),
                    rng.choice(["r1", "r1", "r2"]),
                    patch=rng.choice([None, "p1", "p2"]),
                    reachable=rng.choice([True, False]),
                    author=rng.choice(["a@example.com", "b@example.com"]),
                    authored=rng.choice([None, 1_700_000_000, 1_700_000_100]),
                    subject=rng.choice(["fix", "feat"]),
                    message=message,
                )
            )
        # A cherry-pick trailer may name an earlier commit of the same case.
        for commit in commits:
            if "{ref}" in commit["message"]:
                target = rng.choice(commits)["sha"]
                commit["message"] = commit["message"].replace("{ref}", target[: rng.choice([7, 12])])
        repository = "r1"
        same_repo = [c for c in commits if c["repository_id"] == repository]
        if not same_repo:
            for commit in commits:
                commit["repository_id"] = repository
            same_repo = commits
        new_ids = [c["id"] for c in same_repo if rng.random() < 0.5] or [same_repo[0]["id"]]
        for commit in same_repo:
            if commit["id"] in new_ids:
                commit["reachable"] = True
        existing: list[list[str]] = []
        for orig in same_repo:
            for equiv in new_ids:
                if orig["id"] != equiv and rng.random() < 0.05:
                    existing.append([orig["id"], equiv])
        cases.append(
            equivalence_case(
                name=f"generated:equivalence:{index:03d}",
                origin=f"generated (seed {SEED})",
                repository=repository,
                commits=commits,
                new_ids=new_ids,
                existing=existing,
            )
        )
    return cases


# ---------------------------------------------------------------- carry links


def carry_case(
    name: str,
    origin: str,
    links: list[dict[str, Any]],
    rewritten_tasks: list[str],
) -> dict[str, Any]:
    factory = fresh_session_factory()
    original_sha = sha_of(7)
    with factory() as session:
        session.add(Commit(id="orig", repository_id="r1", sha=original_sha))
        session.add(Commit(id="rew", repository_id="r1", sha=sha_of(8)))
        session.flush()
        for index, link in enumerate(links):
            session.add(
                CommitTaskLink(
                    id=f"link-{index}",
                    commit_id="orig",
                    task_id=link["task_id"],
                    source=link["source"],
                    confidence=link["confidence"],
                    evidence={},
                    attempt_id=link["attempt_id"],
                )
            )
        for task in rewritten_tasks:
            session.add(
                CommitTaskLink(
                    id=f"pre-{task}",
                    commit_id="rew",
                    task_id=task,
                    source="rewrite_reconciliation",
                    confidence=0.85,
                    evidence={},
                )
            )
        session.commit()
    with factory() as session:
        original = session.get(Commit, "orig")
        rewritten = session.get(Commit, "rew")
        GitService._carry_links(session, original, rewritten)
        session.commit()
    with factory() as session:
        carried = session.scalars(
            select(CommitTaskLink).where(
                CommitTaskLink.commit_id == "rew",
                CommitTaskLink.source == "rewrite_reconciliation",
                CommitTaskLink.id.not_like("pre-%"),
            )
        ).all()
        produced = [
            {
                "task_id": row.task_id,
                "confidence": row.confidence,
                "from_commit": row.evidence["from_commit"],
                "original_source": row.evidence["original_source"],
                "attempt_id": row.attempt_id,
            }
            for row in carried
        ]
    produced.sort(key=lambda r: r["task_id"])
    return {
        "id": name,
        "origin": origin,
        "input": {
            "original_sha": original_sha,
            "original_links": links,
            "rewritten_tasks": rewritten_tasks,
        },
        "output": produced,
    }


def carry_cases(rng: random.Random) -> list[dict[str, Any]]:
    cases = [
        carry_case(
            "carry_confidence_capped",
            "hand-written: a trailer link (0.95) is capped at the rewrite confidence",
            [{"task_id": "t1", "source": "trailer", "confidence": 0.95, "attempt_id": None}],
            [],
        ),
        carry_case(
            "carry_low_confidence_kept",
            "hand-written: a lower link keeps its own confidence",
            [{"task_id": "t1", "source": "message_inference", "confidence": 0.35, "attempt_id": "att-1"}],
            [],
        ),
        carry_case(
            "carry_skips_task_already_reconciled",
            "hand-written: the rewritten commit already carries that task",
            [{"task_id": "t1", "source": "trailer", "confidence": 0.95, "attempt_id": None}],
            ["t1"],
        ),
        carry_case(
            "carry_one_task_two_sources",
            "hand-written: a task linked twice on the original is carried once",
            [
                {"task_id": "t1", "source": "trailer", "confidence": 0.95, "attempt_id": None},
                {"task_id": "t1", "source": "explicit", "confidence": 1.0, "attempt_id": None},
            ],
            [],
        ),
        carry_case("carry_no_links", "hand-written: nothing to carry", [], []),
    ]
    sources = [("trailer", 0.95), ("explicit", 1.0), ("worktree", 0.9), ("message_inference", 0.35)]
    for index in range(RANDOM_CARRY_CASES):
        count = rng.randint(0, 4)
        links = []
        for _ in range(count):
            source, confidence = rng.choice(sources)
            links.append(
                {
                    "task_id": f"t{rng.randint(1, 4)}",
                    "source": source,
                    "confidence": confidence,
                    "attempt_id": rng.choice([None, "att-1"]),
                }
            )
        # The table is unique on (commit, task, source), so each pair appears once.
        links = list({(link["task_id"], link["source"]): link for link in links}.values())
        rewritten = sorted({f"t{rng.randint(1, 4)}" for _ in range(rng.randint(0, 2))})
        cases.append(
            carry_case(
                f"generated:carry:{index:03d}",
                f"generated (seed {SEED})",
                links,
                rewritten,
            )
        )
    return cases


# ---------------------------------------------------------------- churn


def churn_case(name: str, origin: str, commits: list[dict[str, Any]]) -> dict[str, Any]:
    factory = fresh_session_factory()
    ids = [c["id"] for c in commits]
    with factory() as session:
        for index, c in enumerate(commits):
            session.add(
                Commit(
                    id=c["id"],
                    repository_id=c["repository_id"],
                    sha=sha_of(1000 + index),
                    patch_id=c["patch_id"],
                    is_merge=c["is_merge"],
                    committed_at=when(c["committed_at"]),
                    insertions=c["insertions"],
                    deletions=c["deletions"],
                )
            )
            if c["analysis"] is not None:
                session.add(
                    CommitAnalysis(
                        commit_id=c["id"],
                        analyzer_version="test",
                        functions_affected=c["analysis"]["functions_affected"],
                        types_affected=c["analysis"]["types_affected"],
                    )
                )
            for fi, f in enumerate(c["files"]):
                session.add(
                    CommitFileChange(
                        id=f"{c['id']}-f{fi}",
                        commit_id=c["id"],
                        path=f["path"],
                        change_type="M",
                        insertions=f["insertions"],
                        deletions=f["deletions"],
                        file_class=file_class(f["path"]),
                    )
                )
        session.commit()
    with factory() as session:
        result = GitService._cumulative_churn(None, session, ids)
    return {
        "id": name,
        "origin": origin,
        "input": {"commits": commits},
        "output": result,
    }


def churn_commit(
    cid: str,
    repository: str,
    *,
    patch: str | None,
    committed_at: int | None,
    insertions: int,
    deletions: int,
    is_merge: bool = False,
    analysis: dict[str, int] | None = None,
    files: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    return {
        "id": cid,
        "repository_id": repository,
        "patch_id": patch,
        "is_merge": is_merge,
        "committed_at": committed_at,
        "insertions": insertions,
        "deletions": deletions,
        "analysis": analysis,
        "files": files or [],
    }


def hand_churn_cases() -> list[dict[str, Any]]:
    src = [{"path": "src/a.py", "insertions": 3, "deletions": 1}]
    return [
        churn_case("churn_empty", "hand-written: no linked commits", []),
        churn_case(
            "churn_single",
            "hand-written: one commit with files and an analysis",
            [
                churn_commit(
                    "c1",
                    "r1",
                    patch="p1",
                    committed_at=100,
                    insertions=3,
                    deletions=1,
                    analysis={"functions_affected": 2, "types_affected": 1},
                    files=src,
                )
            ],
        ),
        churn_case(
            "churn_cherry_pick_counted_once",
            "hand-written: two commits with one patch id count once, the earlier kept",
            [
                churn_commit(
                    "late", "r1", patch="p1", committed_at=200, insertions=3, deletions=1, files=src
                ),
                churn_commit(
                    "early", "r1", patch="p1", committed_at=100, insertions=3, deletions=1, files=src
                ),
            ],
        ),
        churn_case(
            "churn_merge_excluded",
            "hand-written: merges are excluded and counted",
            [
                churn_commit(
                    "m", "r1", patch=None, committed_at=50, insertions=9, deletions=9, is_merge=True
                ),
                churn_commit("c", "r1", patch="p2", committed_at=60, insertions=2, deletions=0, files=src),
            ],
        ),
        churn_case(
            "churn_same_patch_other_repository",
            "hand-written: the same patch in two repositories is two pieces of work",
            [
                churn_commit("a", "r1", patch="p1", committed_at=10, insertions=3, deletions=1, files=src),
                churn_commit("b", "r2", patch="p1", committed_at=20, insertions=3, deletions=1, files=src),
            ],
        ),
        churn_case(
            "churn_undated_commit_last",
            "hand-written: a commit without a date sorts after the dated ones",
            [
                churn_commit("undated", "r1", patch=None, committed_at=None, insertions=1, deletions=0),
                churn_commit("dated", "r1", patch="p9", committed_at=5, insertions=2, deletions=0),
            ],
        ),
        churn_case(
            "churn_repeated_file_edits",
            "hand-written: a path touched by two counted commits is a repeated edit",
            [
                churn_commit(
                    "x",
                    "r1",
                    patch="p1",
                    committed_at=1,
                    insertions=4,
                    deletions=2,
                    files=[
                        {"path": "tests/t.py", "insertions": 2, "deletions": 0},
                        {"path": "docs/readme.md", "insertions": 2, "deletions": 2},
                    ],
                ),
                churn_commit(
                    "y",
                    "r1",
                    patch="p2",
                    committed_at=2,
                    insertions=1,
                    deletions=0,
                    analysis={"functions_affected": 1, "types_affected": 0},
                    files=[{"path": "tests/t.py", "insertions": 1, "deletions": 0}],
                ),
            ],
        ),
    ]


def random_churn_cases(rng: random.Random) -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    paths = ["src/a.py", "src/b.rs", "tests/test_a.py", "docs/x.md", "pkg/__init__.py", "web/app.tsx"]
    for index in range(RANDOM_CHURN_CASES):
        commits: list[dict[str, Any]] = []
        for n in range(rng.randint(0, 7)):
            # An undated commit is never given a patch id, so the "now" stand-in never decides a duplicate.
            patch = rng.choice([None, "p1", "p2", "p3", ""])
            # Distinct dates per commit, so duplicates never tie on the sort key.
            committed = 1_700_000_000 + index * 1000 + n * 10 + rng.randint(0, 9)
            files = [
                {
                    "path": rng.choice(paths),
                    "insertions": rng.randint(0, 9),
                    "deletions": rng.randint(0, 9),
                }
                for _ in range(rng.randint(0, 3))
            ]
            files = list({f["path"]: f for f in files}.values())
            analysis = (
                {"functions_affected": rng.randint(0, 5), "types_affected": rng.randint(0, 2)}
                if rng.random() < 0.5
                else None
            )
            commits.append(
                churn_commit(
                    f"c{n}",
                    rng.choice(["r1", "r2"]),
                    patch=patch,
                    committed_at=committed if patch else rng.choice([committed, None]),
                    insertions=rng.randint(0, 20),
                    deletions=rng.randint(0, 20),
                    is_merge=rng.random() < 0.15,
                    analysis=analysis,
                    files=files,
                )
            )
        cases.append(churn_case(f"generated:churn:{index:03d}", f"generated (seed {SEED})", commits))
    return cases


# ---------------------------------------------------------------- change surface (replica)


def replica_surface(case: dict[str, Any]) -> dict[str, Any]:
    """``compute_change_surface`` for one repository, with its git questions answered from the case.

    Copies the decision and record code of ``GitService.compute_change_surface``
    (``integrations/git/service.py``) line for line; only the calls into ``Git`` and
    ``analyze_file`` are replaced by the case data.
    """
    exists_set = set(case["exists"])
    ancestors = {tuple(pair) for pair in case["ancestors"]}
    parents = case["parents"]

    def object_exists(sha: str) -> bool:
        return sha in exists_set

    def is_ancestor(a: str, b: str) -> bool:
        return (a, b) in ancestors

    base = case["manifest_base"] or None
    final = case["manifest_final"] or None
    provenance = {
        "base": "attempt baseline manifest" if base else None,
        "final": "accepted revision manifest" if final else None,
    }
    linked = case["linked"]
    if (base is None or final is None) and linked:
        shas = [s for s in linked if object_exists(s)]
        if not shas:
            return {"plan": "skip"}
        if final is None:
            final = next((s for s in shas if all(is_ancestor(o, s) for o in shas)), shas[-1])
            provenance["final"] = "latest linked commit"
        if base is None:
            earliest = next((s for s in shas if all(is_ancestor(s, o) for o in shas)), shas[0])
            parent = parents.get(earliest) or ""
            base = parent or EMPTY_TREE
            provenance["base"] = "parent of earliest linked commit"
    if not base or not final or base == final:
        return {"plan": "skip"}
    if not (object_exists(final) and (object_exists(base) or base == EMPTY_TREE)):
        return {"plan": "gap", "base": base, "final_rev": final}

    functions = types = 0
    fallback_files = 0
    numstat = case["files"]
    for index, f in enumerate(numstat):
        if f["binary"]:
            continue
        structure = case["structures"][index]
        if structure is not None:
            functions += structure[0]
            types += structure[1]
        else:
            fallback_files += 1
    record = {
        "base_sha": base,
        "final_sha": final,
        "files_changed": len(numstat),
        "insertions": sum(f["insertions"] for f in numstat),
        "deletions": sum(f["deletions"] for f in numstat),
        "renames": sum(1 for f in numstat if f["old_path"]),
        "binary_files": sum(1 for f in numstat if f["binary"]),
        "functions_affected": functions,
        "types_affected": types,
        "detail": {
            "provenance": provenance,
            "structural_fallback_files": fallback_files,
            "files": [
                {
                    "path": f["path"],
                    "old_path": f["old_path"],
                    "insertions": f["insertions"],
                    "deletions": f["deletions"],
                    "binary": f["binary"],
                    "class": file_class(f["path"]),
                }
                for f in numstat[:500]
            ],
        },
    }
    return {
        "plan": "measure",
        "base": base,
        "final_rev": final,
        "provenance": provenance,
        "record": record,
    }


def surface_case(name: str, origin: str, case: dict[str, Any]) -> dict[str, Any]:
    return {"id": name, "origin": origin, "input": case, "output": replica_surface(case)}


def file_row(
    path: str, rng: random.Random | None = None, *, old: str | None = None, binary: bool = False
) -> dict[str, Any]:
    if rng is None:
        return {"path": path, "old_path": old, "insertions": 2, "deletions": 1, "binary": binary}
    return {
        "path": path,
        "old_path": old,
        "insertions": rng.randint(0, 30),
        "deletions": rng.randint(0, 30),
        "binary": binary,
    }


def surface_base(**overrides: Any) -> dict[str, Any]:
    case: dict[str, Any] = {
        "manifest_base": None,
        "manifest_final": None,
        "linked": [],
        "exists": [],
        "ancestors": [],
        "parents": {},
        "files": [],
        "structures": [],
    }
    case.update(overrides)
    return case


def hand_surface_cases() -> list[dict[str, Any]]:
    b, f, p = sha_of(20), sha_of(21), sha_of(22)
    l1, l2 = sha_of(30), sha_of(31)
    files = [
        file_row("src/core.py"),
        file_row("src/new.py", old="src/old.py"),
        file_row("img/logo.png", binary=True),
        file_row("tests/test_core.py"),
    ]
    structures = [[3, 1], None, None, [0, 0]]
    manifests = surface_base(
        manifest_base=b,
        manifest_final=f,
        exists=[b, f],
        files=files,
        structures=structures,
    )
    cases = [
        surface_case("manifests_measured", "hand-written: both revisions from manifests", manifests),
        surface_case(
            "manifests_equal_skipped",
            "hand-written: equal base and final measure nothing",
            surface_base(manifest_base=b, manifest_final=b, exists=[b]),
        ),
        surface_case(
            "manifest_object_missing_gap",
            "hand-written: a missing final object is a gap",
            surface_base(manifest_base=b, manifest_final=f, exists=[b], files=files, structures=structures),
        ),
        surface_case(
            "no_manifests_no_links_skipped",
            "hand-written: nothing known about the repository",
            surface_base(),
        ),
        surface_case(
            "final_from_latest_linked_base_from_parent",
            "hand-written: final is the latest linked commit, base the parent of the earliest",
            surface_base(
                linked=[l1, l2],
                exists=[l1, l2, p],
                ancestors=[[l1, l1], [l2, l2], [l1, l2]],
                parents={l1: p},
                files=files,
                structures=structures,
            ),
        ),
        surface_case(
            "root_commit_uses_empty_tree",
            "hand-written: the earliest linked commit has no parent, so the base is the empty tree",
            surface_base(
                linked=[l1],
                exists=[l1],
                ancestors=[[l1, l1]],
                parents={},
                files=files,
                structures=structures,
            ),
        ),
        surface_case(
            "no_existing_linked_commit_skipped",
            "hand-written: linked commits whose objects are gone leave nothing to measure",
            surface_base(linked=[l1], exists=[], ancestors=[], parents={}),
        ),
        surface_case(
            "base_manifest_final_from_links",
            "hand-written: a manifest base with the final taken from the linked commits",
            surface_base(
                manifest_base=b,
                linked=[l1],
                exists=[b, l1],
                ancestors=[[l1, l1]],
                files=files,
                structures=structures,
            ),
        ),
        surface_case(
            "missing_base_object_gap",
            "hand-written: a missing base object is a gap, unless it is the empty tree",
            surface_base(
                manifest_base=b,
                manifest_final=f,
                exists=[f],
                files=files,
                structures=structures,
            ),
        ),
    ]
    big_files = [file_row(f"pkg/m{i:03d}.py") for i in range(510)]
    big_structures = [[1, 0] if i % 3 else None for i in range(510)]
    cases.append(
        surface_case(
            "detail_truncated_to_500",
            "hand-written: 510 files, the detail keeps the first 500",
            surface_base(
                manifest_base=b,
                manifest_final=f,
                exists=[b, f],
                files=big_files,
                structures=big_structures,
            ),
        )
    )
    return cases


def random_surface_cases(rng: random.Random) -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    for index in range(RANDOM_SURFACE_CASES):
        chain = [sha_of(index * 100 + n + 1) for n in range(rng.randint(1, 4))]
        stray = sha_of(index * 100 + 90)
        universe = [*chain, stray]
        exists = [s for s in universe if rng.random() < 0.8]
        ancestors = []
        for i, a in enumerate(chain):
            for j in range(i, len(chain)):
                ancestors.append([a, chain[j]])
        ancestors.append([stray, stray])
        parents = {s: rng.choice(universe) for s in universe if rng.random() < 0.7}
        linked = [s for s in chain if rng.random() < 0.8]
        rng.shuffle(linked)
        base_choice = rng.choice([None, None, sha_of(index * 100 + 95)])
        final_choice = rng.choice([None, None, chain[-1]])
        files = [
            file_row(
                rng.choice(["src/a.py", "src/b.rs", "docs/c.md", "tests/t.py"]),
                rng,
                old=rng.choice([None, None, "src/old.py"]),
                binary=rng.random() < 0.15,
            )
            for _ in range(rng.randint(0, 5))
        ]
        structures = [None if rng.random() < 0.3 else [rng.randint(0, 6), rng.randint(0, 3)] for _ in files]
        case = surface_base(
            manifest_base=base_choice,
            manifest_final=final_choice,
            linked=linked,
            exists=exists,
            ancestors=ancestors,
            parents=parents,
            files=files,
            structures=structures,
        )
        cases.append(surface_case(f"generated:surface:{index:03d}", f"generated (seed {SEED})", case))
    return cases


# ---------------------------------------------------------------- output


def write(name: str, function: str, origin: str, cases: list[dict[str, Any]]) -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    payload = {"function": function, "origin": origin, "cases": cases}
    text = json.dumps(payload, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    with (OUT_DIR / name).open("w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def main() -> None:
    rng = random.Random(SEED)  # noqa: S311 - test-data generation, not security

    equivalences = hand_equivalence_cases() + random_equivalence_cases(rng)
    write(
        "equivalences.json",
        "_detect_equivalences",
        "GitService._detect_equivalences run against an in-memory database (real Python code)",
        equivalences,
    )
    write(
        "carry_links.json",
        "_carry_links",
        "GitService._carry_links run against an in-memory database (real Python code)",
        carry_cases(rng),
    )
    write(
        "churn.json",
        "_cumulative_churn",
        "GitService._cumulative_churn run against an in-memory database (real Python code)",
        hand_churn_cases() + random_churn_cases(rng),
    )
    write(
        "change_surface.json",
        "compute_change_surface",
        "replica of GitService.compute_change_surface with git answered from the case data",
        hand_surface_cases() + random_surface_cases(rng),
    )


if __name__ == "__main__":
    main()
