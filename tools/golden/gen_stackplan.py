"""Golden vectors GV-21 (stackplan): the pure parts of the stack planner.

Covered, with their Python sources:

* queue order and the merge sequence: ``StackVerifier.item_rows`` and ``StackVerifier._sequence``
  (``integrations/git/stack_verification.py``);
* the sibling choice for a child's base: ``builds_on_others`` inside ``stack_base``
  (``core/services/stacks.py``);
* prefix construction for bisection: ``_prefix_shas``, the bisection search loop of ``_bisect``, and
  the verify and bisect entry decisions (``verify``, ``_bisect``), with ``_failed_checks``,
  ``_errored_checks`` and ``_is_absolute`` (``integrations/git/stack_verification.py``);
* regression judgement: ``regressions`` (``integrations/git/regression.py``);
* landing preconditions: the ordered checks of ``StackLander._land``
  (``integrations/git/stack_landing.py``).

Module-level helpers are called directly. Logic that lives inside a service method (queue order,
``builds_on_others``, the decision sequences) is transcribed here statement by statement from the
named source, and each transcription names its origin. Database rows and Git refs are replaced by
generated in-memory values; no repository, database or real Workgraph data is read.

Writes ``golden/stackplan/<name>.json`` at the repository root. Output is sorted-key JSON with
``\\n`` line endings, so two runs produce identical bytes.
"""

from __future__ import annotations

import json
import random
from collections.abc import Callable
from datetime import UTC, datetime
from itertools import combinations
from pathlib import Path
from typing import Any

from workgraph.core.models.enums import StackItemState, StackStatus, TaskStatus
from workgraph.integrations.git.regression import CheckOutcome, regressions
from workgraph.integrations.git.stack_integration import load_detail
from workgraph.integrations.git.stack_landing import ACCEPTABLE_TASK_STATUSES
from workgraph.integrations.git.stack_verification import (
    ON_BRANCH_STATES,
    _Entry,
    _errored_checks,
    _failed_checks,
    _is_absolute,
    _prefix_shas,
)
from workgraph.verification.configs import CheckSpec
from workgraph.verification.service import CheckResultOut, VerificationRunOut

SEED = 21
REPOS = ["alpha", "beta", "gamma"]
ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "golden" / "stackplan"
CREATED = datetime(2026, 1, 1, tzinfo=UTC)


def dumps(value: Any) -> str:
    return json.dumps(value, sort_keys=True, indent=2, ensure_ascii=False) + "\n"


def merges_for(rng: random.Random) -> dict[str, dict[str, str]]:
    """A random, possibly empty, set of merges keyed by repository name."""
    chosen = rng.sample(REPOS, rng.randint(0, len(REPOS)))
    return {name: {"merge_commit": f"m{rng.randrange(10**6):06d}"} for name in sorted(chosen)}


# ---------------------------------------------------------------- queue order


def ordered_items(items: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """``StackVerifier.item_rows`` ORDER BY merge_order IS NULL, merge_order, Task.number."""
    return sorted(
        items,
        key=lambda item: (item["merge_order"] is None, item["merge_order"] or 0, item["number"]),
    )


def merge_sequence(items: list[dict[str, Any]]) -> list[str]:
    """``StackVerifier._sequence``: items on the integration branch that have merges, in order."""
    return [
        item["key"]
        for item in ordered_items(items)
        if StackItemState(item["state"]) in ON_BRANCH_STATES and load_detail(item["detail"])["merges"]
    ]


def gen_queue_case(rng: random.Random, index: int) -> dict[str, Any]:
    numbers = rng.sample(range(1, 60), rng.randint(0, 7))
    items = []
    for number in numbers:
        kind = rng.randrange(7)
        detail: str | None
        if kind == 0:
            detail = None
        elif kind == 1:
            detail = ""
        elif kind == 2:
            detail = "free text, not json"
        elif kind == 3:
            detail = json.dumps({"merges": {}})
        elif kind == 4:
            detail = json.dumps({"merges": merges_for(rng) or {"alpha": {"merge_commit": "m000001"}}})
        elif kind == 5:
            detail = json.dumps([1, 2])
        else:
            detail = json.dumps({"conflicts": {"alpha": "x"}, "merges": {}})
        items.append(
            {
                "key": f"WG-{number}",
                "number": number,
                # Duplicate merge orders and NULLs are both generated; the number breaks ties.
                "merge_order": rng.choice([None, None, rng.randint(0, 4)]),
                "state": rng.choice([state.value for state in StackItemState]),
                "detail": detail,
            }
        )
    rng.shuffle(items)
    return {
        "id": f"queue-{index:03d}",
        "items": items,
        "order": [item["key"] for item in ordered_items(items)],
        "sequence": merge_sequence(items),
    }


# ---------------------------------------------------------- builds_on_others


def gen_commit_dag(rng: random.Random, size: int) -> dict[str, list[str]]:
    """A commit DAG, each commit's parents chosen among earlier commits (merges included)."""
    names = [f"c{i}" for i in range(size)]
    parents: dict[str, list[str]] = {}
    for i, name in enumerate(names):
        pool = names[:i]
        if not pool:
            parents[name] = []
            continue
        count = min(len(pool), rng.choice([1, 1, 1, 2]))
        parents[name] = sorted(rng.sample(pool, count))
    return parents


def reachable(parents: dict[str, list[str]]) -> dict[str, set[str]]:
    """For each commit, itself and every ancestor: ``is_ancestor(a, b)`` is ``a in reachable[b]``."""
    result: dict[str, set[str]] = {}
    for node in parents:
        seen = {node}
        stack = [node]
        while stack:
            current = stack.pop()
            for parent in parents[current]:
                if parent not in seen:
                    seen.add(parent)
                    stack.append(parent)
        result[node] = seen
    return result


def choose_sibling(heads: list[tuple[dict[str, Any], str]], ancestry: dict[str, set[str]]) -> dict[str, Any]:
    """``stack_base``'s sibling branch in core/services/stacks.py, transcribed.

    ``heads`` are (sibling, head) pairs for the siblings that touched the repository, in Task.number
    order. ``is_ancestor(a, b)`` is true when ``a`` is reachable from ``b``, including ``a == b``.
    """
    if not heads:
        return {"source": "stack_base"}

    def is_ancestor(a: str, b: str) -> bool:
        return a in ancestry[b]

    for (first, first_head), (second, second_head) in combinations(heads, 2):
        if not (is_ancestor(first_head, second_head) or is_ancestor(second_head, first_head)):
            return {"source": "error", "error": "ValidationFailed", "siblings": [first["key"], second["key"]]}

    def builds_on_others(entry: tuple[dict[str, Any], str]) -> int:
        return sum(
            1
            for other, other_head in heads
            if other["key"] != entry[0]["key"] and is_ancestor(other_head, entry[1])
        )

    ordered = sorted(heads, key=builds_on_others, reverse=True)
    (latest, head), superseded = ordered[0], ordered[1:]
    return {
        "source": "sibling_head",
        "sibling": latest["key"],
        "sibling_head": head,
        "superseded": [{"task": other["key"], "head": other_head} for other, other_head in superseded],
    }


def gen_sibling_case(rng: random.Random, index: int) -> dict[str, Any]:
    size = rng.randint(2, 10)
    parents = gen_commit_dag(rng, size)
    ancestry = reachable(parents)
    commits = sorted(parents)
    siblings = []
    for number in sorted(rng.sample(range(1, 80), rng.randint(0, 4))):
        # Siblings that did not touch the repository carry no head and are filtered out first.
        head = None if rng.random() < 0.2 else rng.choice(commits)
        siblings.append({"key": f"WG-{number}", "number": number, "head": head})
    touched = [s for s in siblings if s["head"] is not None]
    return {
        "id": f"sibling-{index:03d}",
        "commits": {name: parents[name] for name in commits},
        "siblings": siblings,
        "result": choose_sibling([(s, s["head"]) for s in touched], ancestry),
    }


# ----------------------------------------------------- bisection and prefixes


def gen_prefix_case(rng: random.Random, index: int) -> dict[str, Any]:
    bases = {name: f"b-{name}-{rng.randrange(100):02d}" for name in REPOS}
    sequence = []
    for i in range(rng.randint(0, 8)):
        entry = _Entry(
            item_id=f"item-{i}",
            task_id=f"task-{i}",
            task_key=f"WG-{100 + i}",
            title=f"change {i}",
            merges=merges_for(rng),
        )
        sequence.append(entry)
    prefixes = [_prefix_shas(bases, sequence, position) for position in range(len(sequence) + 1)]
    return {
        "id": f"prefix-{index:03d}",
        "bases": bases,
        "sequence": [{"task": entry.task_key, "merges": entry.merges} for entry in sequence],
        "prefix_shas": prefixes,
    }


def gen_search_case(rng: random.Random, index: int) -> dict[str, Any]:
    """Bisection search as ``StackVerifier._bisect`` runs it, over a generated probe outcome table.

    ``passes[k]`` is whether the probe of prefix ``k`` passes; ``errored[k]`` is whether that probe
    errors. Probes are only taken at 1..n-1, and the tip (n) fails by the precondition of ``_bisect``.
    Outcomes are not monotone, which exercises the loop exactly as written.
    """
    n = rng.randint(1, 10)
    bases = {name: f"b-{name}" for name in REPOS}
    sequence = [
        _Entry(
            f"item-{i}",
            f"task-{i}",
            f"WG-{200 + i}",
            f"change {i}",
            merges_for(rng) or {"alpha": {"merge_commit": f"m{i:06d}"}},
        )
        for i in range(n)
    ]
    passes = [True] + [rng.random() < 0.5 for _ in range(n - 1)] + [False]
    errored = [False] * (n + 1)
    for k in range(1, n):
        errored[k] = rng.random() < 0.08
    low, high = 0, n
    probes: list[dict[str, Any]] = []
    outcome: dict[str, Any]
    while high - low > 1:
        middle = (low + high) // 2
        shas = _prefix_shas(bases, sequence, middle)
        if errored[middle]:
            probes.append({"position": middle, "shas": shas, "errored": True})
            outcome = {"outcome": "errored", "stage": "bisect"}
            break
        passed = passes[middle]
        probes.append({"position": middle, "shas": shas, "passed": passed})
        if passed:
            low = middle
        else:
            high = middle
    else:
        outcome = {"outcome": "ejected", "position": high, "culprit": sequence[high - 1].task_key}
    return {
        "id": f"search-{index:03d}",
        "bases": bases,
        "sequence": [entry.task_key for entry in sequence],
        "passes": passes,
        "errored": errored,
        "probes": probes,
        "result": outcome,
    }


# ------------------------------------------------- regression and verify judgement


CHECK_NAMES = ["cargo", "pytest", "lint"]
SIGNATURES = [
    "test:a::b::test_alpha",
    "test:test_alpha",
    "test:c::test_beta",
    "test:test_gamma",
    "test:d::e::test_delta",
    "build:E0001",
    "timeout",
]
CHECK_STATUSES = ["passed", "failed", "error", "skipped"]


def verify_decision(
    run: VerificationRunOut, base: dict[str, CheckOutcome], specs: dict[str, CheckSpec]
) -> dict[str, Any]:
    """``StackVerifier.verify`` from its ``_errored_checks`` call to the bisect branch, transcribed."""
    errored = _errored_checks(run)
    if errored:
        return {"outcome": "errored", "stage": "tip", "checks": sorted(c.name for c in errored)}
    if run.required_passed:
        return {"outcome": "passed", "reason": "full checks passed"}
    failed = _failed_checks(run)
    base_errors = sorted(name for name, outcome in base.items() if outcome.status == "error")
    if base_errors:
        return {"outcome": "errored", "stage": "base", "checks": base_errors}
    if not regressions(run, base, specs):
        tolerated = {
            check.name: list(check.failure_signatures) for check in run.checks if check.name in failed
        }
        return {"outcome": "passed", "reason": "no new failures against the base", "tolerated": tolerated}
    return {"outcome": "bisect", "failing": sorted(regressions(run, base, specs).items())}


def bisect_entry(
    sequence_len: int, tip_run: VerificationRunOut, base: dict[str, CheckOutcome], specs: dict[str, CheckSpec]
) -> dict[str, Any]:
    """The start of ``StackVerifier._bisect``: the base-failure branch, transcribed."""
    failing = regressions(tip_run, base, specs)
    failed = _failed_checks(tip_run)
    on_base = sorted(
        name
        for name in failed
        if _is_absolute(specs, name) and (name not in base or base[name].status != "passed")
    )
    if sequence_len == 0 or on_base:
        reason = "no merged change to eject" if sequence_len == 0 else "the base fails"
        return {"outcome": "failed_on_base", "reason": reason, "failed_checks": sorted(on_base or failing)}
    return {"outcome": "search"}


def gen_judgement_case(rng: random.Random, index: int) -> dict[str, Any]:
    names = rng.sample(CHECK_NAMES, rng.randint(1, 3))
    checks: list[dict[str, Any]] = []
    for name in names:
        checks.append(
            {
                "name": name,
                "required": rng.random() < 0.85,
                "status": rng.choices(CHECK_STATUSES, weights=[5, 3, 1, 1])[0],
                "repository_id": rng.choice([None, "r1"]),
                "failure_signatures": rng.sample(SIGNATURES, rng.randint(0, 3)),
            }
        )
    spec_baselines: dict[str, str] = {}
    for name in names:
        if rng.random() < 0.7:
            spec_baselines[name] = rng.choice(["absolute", "regression"])
    base_inputs: dict[str, dict[str, Any]] = {}
    for name in names:
        if rng.random() < 0.6:
            base_inputs[name] = {
                "status": rng.choice(["passed", "failed", "error"]),
                "signatures": rng.sample(SIGNATURES, rng.randint(0, 3)),
            }

    specs = {
        name: CheckSpec(name=name, command=["run"], baseline=baseline)  # type: ignore[arg-type]
        for name, baseline in spec_baselines.items()
    }
    base = {
        name: CheckOutcome(status=data["status"], signatures=data["signatures"], run_id="base-1")
        for name, data in base_inputs.items()
    }
    run = VerificationRunOut(
        id="run-1",
        workspace_id="ws-1",
        task_id="task-1",
        attempt_id=None,
        manifest_id=None,
        status="failed",
        required_passed=all(c["status"] == "passed" for c in checks if c["required"]),
        triggered_by="stack",
        started_at=None,
        finished_at=None,
        created_at=CREATED,
        checks=[
            CheckResultOut(
                id=f"chk-{i}",
                name=c["name"],
                kind="test",
                required=c["required"],
                repository_id=c["repository_id"],
                status=c["status"],
                exit_code=None,
                duration_ms=None,
                timed_out=False,
                log_artifact_id=None,
                output_excerpt="",
                failure_signatures=c["failure_signatures"],
            )
            for i, c in enumerate(checks)
        ],
    )
    sequence_len = rng.randint(0, 3)
    return {
        "id": f"judge-{index:03d}",
        "checks": checks,
        "spec_baselines": spec_baselines,
        "base": base_inputs,
        "sequence_len": sequence_len,
        "regressions": dict(sorted(regressions(run, base, specs).items())),
        "failed": dict(sorted(_failed_checks(run).items())),
        "errored": sorted(c.name for c in _errored_checks(run)),
        "verify": verify_decision(run, base, specs),
        "bisect_entry": bisect_entry(sequence_len, run, base, specs),
    }


# ---------------------------------------------------------- landing preconditions


def land_decision(state: dict[str, Any]) -> dict[str, Any]:
    """The ordered checks of ``StackLander._land`` and the branch taken, transcribed.

    Errors are reported as the Python exception class and a code naming the check; message text is
    not vectored.
    """

    def refused(kind: str, code: str, **extra: Any) -> dict[str, Any]:
        return {"outcome": "refused", "error": {"kind": kind, "code": code}, **extra}

    if StackStatus(state["stack_status"]) != StackStatus.VERIFIED:
        return refused("ConflictError", "stack_not_verified")
    if TaskStatus(state["parent_status"]) not in ACCEPTABLE_TASK_STATUSES:
        return refused("ConflictError", "parent_not_acceptable")

    ordered = ordered_items(state["items"])
    partial = sorted(
        f"{item['key']} ({item['state']})"
        for item in ordered
        if StackItemState(item["state"]) != StackItemState.VERIFIED and load_detail(item["detail"])["merges"]
    )
    if partial:
        return refused("ConflictError", "unverified_children_with_merges", unfinished=partial)

    children = []
    for item in ordered:
        if StackItemState(item["state"]) != StackItemState.VERIFIED:
            continue
        if TaskStatus(item["task_status"]) not in ACCEPTABLE_TASK_STATUSES:
            return refused("ConflictError", "child_not_acceptable")
        children.append(item["key"])

    repos = sorted(state["repos"], key=lambda repo: repo["name"])
    defaults = {repo["id"]: repo["default_branch"] for repo in state["repos"]}
    if not state["verified_event"]:
        return refused("ConflictError", "no_verification_event")
    tips = state["tips"]
    for repo in repos:
        if not defaults.get(repo["id"]):
            return refused("ValidationFailed", "no_default_branch")
        if repo["integrated_tip"] != tips.get(repo["name"]):
            return refused("ConflictError", "integration_moved")

    heads = {repo["name"]: repo["default_head"] for repo in repos}
    if any(heads[repo["name"]] != repo["base_sha"] for repo in repos):
        return {"outcome": "deferred", "children": children}
    return {"outcome": "land", "children": children}


def gen_landing_case(rng: random.Random, index: int) -> dict[str, Any]:
    repo_names = sorted(rng.sample(REPOS, rng.randint(1, 3)))
    repos = []
    for name in repo_names:
        base_sha = f"b-{name}"
        repos.append(
            {
                "name": name,
                "id": f"id-{name}",
                "default_branch": None if rng.random() < 0.1 else "main",
                "base_sha": base_sha,
                "integrated_tip": f"t-{name}" if rng.random() < 0.8 else f"x-{name}",
                "default_head": base_sha if rng.random() < 0.8 else f"moved-{name}",
            }
        )
    tips = {repo["name"]: f"t-{repo['name']}" for repo in repos if rng.random() < 0.9}
    numbers = rng.sample(range(1, 90), rng.randint(0, 4))
    items = []
    for number in numbers:
        items.append(
            {
                "key": f"WG-{number}",
                "number": number,
                "merge_order": rng.choice([None, rng.randint(0, 3)]),
                "state": rng.choices([s.value for s in StackItemState], weights=[1, 1, 2, 1, 1, 6])[0],
                "task_status": rng.choices(
                    [t.value for t in TaskStatus],
                    weights=[1 if t not in ACCEPTABLE_TASK_STATUSES else 6 for t in TaskStatus],
                )[0],
                "detail": json.dumps({"merges": merges_for(rng)}),
            }
        )
    state = {
        "stack_status": rng.choices([s.value for s in StackStatus], weights=[1, 1, 1, 8, 1, 1])[0],
        "parent_status": rng.choices(
            [t.value for t in TaskStatus],
            weights=[1 if t not in ACCEPTABLE_TASK_STATUSES else 8 for t in TaskStatus],
        )[0],
        "items": items,
        "verified_event": rng.random() < 0.9,
        "repos": repos,
        "tips": tips,
    }
    return {"id": f"land-{index:03d}", "state": state, "result": land_decision(state)}


# ---------------------------------------------------------------- output


def write_group(name: str, cases: list[dict[str, Any]]) -> dict[str, Any]:
    body = dumps({"group": name, "generator": "tools/golden/gen_stackplan.py", "seed": SEED, "cases": cases})
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / f"{name}.json").write_bytes(body.encode("utf-8"))
    return {"file": f"golden/stackplan/{name}.json", "cases": len(cases), "bytes": len(body.encode("utf-8"))}


def main() -> None:
    rng = random.Random(SEED)  # noqa: S311 - seeded test-data generator, not a security use
    groups: list[tuple[str, Callable[[random.Random, int], dict[str, Any]], int]] = [
        ("queue_order", gen_queue_case, 300),
        ("builds_on_others", gen_sibling_case, 300),
        ("prefix_shas", gen_prefix_case, 200),
        ("bisection_search", gen_search_case, 300),
        ("regression_judgement", gen_judgement_case, 400),
        ("landing_preconditions", gen_landing_case, 400),
    ]
    summary = []
    for name, gen, count in groups:
        cases = [gen(rng, i) for i in range(count)]
        summary.append(write_group(name, cases))
    for row in summary:
        print(f"{row['file']}: {row['cases']} cases, {row['bytes']} bytes")


if __name__ == "__main__":
    main()
