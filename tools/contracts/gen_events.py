"""Regenerate ``contracts/events.json``, the domain event vocabulary (task CF-06).

Inputs:

* the ``event_type=`` call sites under ``src/workgraph``, read with the ``ast`` module (read only);
* the events that a fresh database records when ``workgraph demo e2e`` runs in an isolated home. The
  run is made here, in a temporary directory, so the script needs nothing committed beyond the source.
  Only event types and data key names are taken from it, never ids or timestamps;
* the entries in ``MANUAL`` below, for call sites whose event type or data dict is computed at run
  time (for example ``f"attempt.{event.kind}"`` or a dict chosen by status). Each entry is checked
  against the code when it is edited.

Output: for every event type, its entity types, the data keys always present (``data_keys``), the
keys that may be present (``optional_data_keys``), and ``data_open`` when callers may merge further
keys into ``data``.

Usage (from the repository root)::

    uv run python tools/contracts/gen_events.py          # rewrite contracts/events.json
    uv run python tools/contracts/gen_events.py --check  # exit 1 if the committed file is stale

Running it twice gives identical bytes.
"""

from __future__ import annotations

import ast
import json
import sqlite3
import subprocess
import sys
import tempfile
from collections import defaultdict
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[2]
SOURCE = REPO / "src" / "workgraph"
OUTPUT = REPO / "contracts" / "events.json"

# event type -> entry. ``data_keys`` are always present; ``optional_data_keys`` may be present;
# ``open`` means callers add keys that are not enumerated here.
MANUAL: dict[str, dict[str, Any]] = {
    "attempt.succeeded": {
        "entity_types": ["attempt"],
        "data_keys": ["duration_ms", "error", "exit_code", "exit_reason", "from", "status", "task_id"],
    },
    "attempt.failed": {
        "entity_types": ["attempt"],
        "data_keys": ["duration_ms", "error", "exit_code", "exit_reason", "from", "status", "task_id"],
    },
    "attempt.cancelled": {
        "entity_types": ["attempt"],
        "data_keys": ["duration_ms", "error", "exit_code", "exit_reason", "from", "status", "task_id"],
    },
    "task.decision_requested": {
        "entity_types": ["task"],
        "data_keys": ["decision_id", "key", "question", "version"],
    },
    "task.decision_chosen": {
        "entity_types": ["task"],
        "data_keys": ["decision_id", "key", "question", "version"],
    },
    "task.decision_withdrawn": {
        "entity_types": ["task"],
        "data_keys": ["decision_id", "key", "question", "version"],
    },
    "task.comment_added": {
        "entity_types": ["task"],
        "data_keys": ["comment_id", "excerpt", "kind"],
        "open": True,
    },
    "task.progress_reported": {
        "entity_types": ["task"],
        "data_keys": ["comment_id", "excerpt", "kind"],
        "open": True,
    },
    "task.result_reported": {
        "entity_types": ["task"],
        "data_keys": ["comment_id", "excerpt", "kind"],
        "open": True,
    },
    "agent_session.subagent_started": {
        "entity_types": ["agent_session"],
        "data_keys": ["agent_id", "agent_type", "child_attempt_id"],
    },
    "agent_session.subagent_stopped": {
        "entity_types": ["agent_session"],
        "data_keys": ["agent_id", "agent_type", "child_attempt_id"],
    },
    "agent_session.internal_task_created": {
        "entity_types": ["agent_session"],
        "data_keys": ["authoritative", "external_task_id", "mapped_task_id", "subject"],
    },
    "agent_session.internal_task_completed": {
        "entity_types": ["agent_session"],
        "data_keys": ["authoritative", "external_task_id", "mapped_task_id", "subject"],
    },
    "git.sync_completed": {
        "entity_types": ["repository"],
        "data_keys": [
            "equivalences",
            "head_refs",
            "links_created",
            "marked_unreachable",
            "mode",
            "new_commits",
            "refs_changed",
            "repository",
            "repository_id",
            "restored_reachable",
            "shallow",
        ],
    },
    "stack.rebased": {
        "entity_types": ["stack"],
        "data_keys": ["base_shas", "new_base_shas", "parent_key", "reason"],
    },
    "stack.land_requested": {
        "entity_types": ["stack"],
        "data_keys": ["base_shas", "mode", "new_base_shas", "parent_key", "reason"],
    },
    "stack.verified": {
        "entity_types": ["stack"],
        "data_keys": ["reason", "run_id", "tasks", "tips"],
        "optional_data_keys": ["tolerated"],
    },
    "stack.item_ejected": {
        "entity_types": ["stack"],
        "data_keys": ["task_key"],
        "optional_data_keys": [
            "failed_checks",
            "manual",
            "position",
            "reason",
            "signatures",
            "verification_run_id",
        ],
    },
    "worktree.recovered": {
        "entity_types": ["system"],
        "data_keys": ["activated", "orphaned", "released", "unknown_dirs"],
    },
    "verification.started": {
        "entity_types": ["verification_run"],
        "data_keys": ["checks", "manifest_id", "task_id", "task_key", "tier"],
        "optional_data_keys": ["stack"],
    },
    "verification.completed": {
        "entity_types": ["verification_run"],
        "data_keys": [
            "failed_checks",
            "optional_failures",
            "required_passed",
            "status",
            "task_id",
            "task_key",
        ],
        "optional_data_keys": ["stack"],
    },
}
# attempt.<kind>: runs.py emits ``attempt.{event.kind}``; the kinds are the AgentEvent literal
# in src/workgraph/integrations/agents/base.py.
for _kind in [
    "session",
    "progress",
    "message",
    "tool_use",
    "tool_result",
    "usage",
    "log",
    "artifact",
    "result",
]:
    MANUAL[f"attempt.{_kind}"] = {"entity_types": ["attempt"], "data_keys": ["message"], "open": True}

RESERVED = {"_automation": "present on any event emitted inside an automation run (depth and actor)"}


# ---------------------------------------------------------------- source scan


def _strings(node: ast.expr | None) -> tuple[set[str], bool]:
    """Event-type or entity-type strings a expression can take, and whether it is unresolved."""
    if node is None:
        return set(), True
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return {node.value}, False
    if isinstance(node, ast.JoinedStr):
        parts = [
            v.value if isinstance(v, ast.Constant) else "{" + ast.unparse(v.value) + "}"  # type: ignore[attr-defined]
            for v in node.values
        ]
        return {"".join(str(p) for p in parts)}, True
    if isinstance(node, ast.IfExp):
        a, ua = _strings(node.body)
        b, ub = _strings(node.orelse)
        return a | b, ua or ub
    if isinstance(node, ast.Dict):
        out: set[str] = set()
        unresolved = False
        for value in node.values:
            s, u = _strings(value)
            out |= s
            unresolved = unresolved or u
        return out, unresolved
    return set(), True


def _data_keys(node: ast.expr | None, dicts: dict[str, set[str]]) -> tuple[set[str], bool]:
    """Literal keys of a ``data=`` argument, and whether more keys can be merged in."""
    if node is None:
        return set(), False
    if isinstance(node, ast.Dict):
        keys: set[str] = set()
        unresolved = False
        for key, value in zip(node.keys, node.values, strict=True):
            if key is None:
                sub, su = _data_keys(value, dicts)
                keys |= sub
                unresolved = unresolved or su
            elif isinstance(key, ast.Constant) and isinstance(key.value, str):
                keys.add(key.value)
            else:
                unresolved = True
        return keys, unresolved
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == "dict":
        keys = {k.arg for k in node.keywords if k.arg}
        return keys, any(k.arg is None for k in node.keywords)
    if isinstance(node, ast.Name) and node.id in dicts:
        return set(dicts[node.id]), False
    return set(), True


def _scan_file(path: Path) -> list[dict[str, Any]]:
    tree = ast.parse(path.read_text(encoding="utf-8"))
    functions = [n for n in ast.walk(tree) if isinstance(n, ast.FunctionDef | ast.AsyncFunctionDef)]

    def local_env(lineno: int) -> tuple[dict[str, set[str]], dict[str, set[str]]]:
        scope: ast.AST = tree
        for func in functions:
            if func.lineno <= lineno <= func.end_lineno and (
                scope is tree or func.lineno >= scope.lineno  # type: ignore[attr-defined]
            ):
                scope = func
        strings: dict[str, set[str]] = {}
        dicts: dict[str, set[str]] = {}
        for node in ast.walk(scope):
            if (
                isinstance(node, ast.Assign)
                and len(node.targets) == 1
                and isinstance(node.targets[0], ast.Name)
            ):
                name = node.targets[0].id
                values, unresolved = _strings(node.value)
                if values and not unresolved:
                    strings.setdefault(name, set()).update(values)
                if isinstance(node.value, ast.Dict):
                    dicts[name], _ = _data_keys(node.value, dicts)
        return strings, dicts

    found: list[dict[str, Any]] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        kw = {k.arg: k.value for k in node.keywords if k.arg}
        if "event_type" not in kw:
            continue
        strings, dicts = local_env(node.lineno)
        event_types, event_unresolved = _strings_env(kw["event_type"], strings)
        entities, _ = _strings_env(kw.get("entity_type"), strings)
        keys, data_unresolved = _data_keys(kw.get("data"), dicts)
        found.append(
            {
                "event_types": sorted(event_types),
                "event_unresolved": event_unresolved,
                "entity_types": sorted(entities),
                "data_keys": sorted(keys),
                "data_unresolved": data_unresolved,
            }
        )
    return found


def _strings_env(node: ast.expr | None, env: dict[str, set[str]]) -> tuple[set[str], bool]:
    if isinstance(node, ast.Name):
        if node.id in env:
            return set(env[node.id]), False
        return set(), True
    return _strings(node)


def scan_sources() -> list[dict[str, Any]]:
    sites: list[dict[str, Any]] = []
    for path in sorted(SOURCE.rglob("*.py")):
        sites.extend(_scan_file(path))
    return sites


# ---------------------------------------------------------------- demo database


def record_demo_events() -> dict[str, dict[str, Any]]:
    """Run the offline demo in a scratch home and return what its events table holds."""
    with tempfile.TemporaryDirectory(prefix="wg-events-") as tmp:
        home = Path(tmp) / "home"
        run = subprocess.run(
            [sys.executable, "-m", "workgraph", "demo", "e2e", "--home", str(home), "--keep"],
            cwd=REPO,
            capture_output=True,
            text=True,
            check=False,
        )
        if run.returncode != 0:
            raise SystemExit(f"workgraph demo e2e failed:\n{run.stdout}\n{run.stderr}")
        conn = sqlite3.connect(home / "data" / "workgraph.db")
        observed: dict[str, dict[str, Any]] = defaultdict(lambda: {"entity_types": set(), "keys": set()})
        for event_type, entity_type, data in conn.execute("SELECT event_type, entity_type, data FROM events"):
            observed[event_type]["entity_types"].add(entity_type)
            payload = json.loads(data) if isinstance(data, str) else (data or {})
            if isinstance(payload, dict):
                observed[event_type]["keys"] |= set(payload)
        conn.close()
    return dict(observed)


# ---------------------------------------------------------------- build


def build() -> str:
    sites = scan_sources()
    observed = record_demo_events()

    by_type: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for site in sites:
        for event_type in site["event_types"]:
            # An f-string with a computed part (``attempt.{event.kind}``) is a family, listed in MANUAL.
            if "{" not in event_type:
                by_type[event_type].append(site)

    events: dict[str, dict[str, Any]] = {}
    for event_type in sorted(set(by_type) | set(MANUAL) | set(observed)):
        if event_type in MANUAL:
            spec = MANUAL[event_type]
            always = set(spec["data_keys"])
            optional = set(spec.get("optional_data_keys", []))
            entities = set(spec["entity_types"])
            is_open = bool(spec.get("open"))
        else:
            entities, is_open, optional = set(), False, set()
            common: set[str] | None = None
            for site in by_type.get(event_type, []):
                entities |= set(site["entity_types"])
                keys = set(site["data_keys"])
                common = keys if common is None else common & keys
                is_open = is_open or site["data_unresolved"]
            always = common or set()
            for site in by_type.get(event_type, []):
                optional |= set(site["data_keys"]) - always
        if event_type in observed:
            entities |= observed[event_type]["entity_types"]
            optional |= observed[event_type]["keys"] - always
        optional = {k for k in optional - always if not k.startswith("_")}
        events[event_type] = {
            "entity_types": sorted(entities),
            "data_keys": sorted(always),
            "optional_data_keys": sorted(optional),
            "data_open": is_open,
        }

    document = {"count": len(events), "events": events, "reserved_data_keys": RESERVED}
    return json.dumps(document, indent=2, sort_keys=True, ensure_ascii=False) + "\n"


def main(argv: list[str]) -> int:
    check = "--check" in argv
    text = build()
    if check:
        current = OUTPUT.read_bytes().decode("utf-8") if OUTPUT.exists() else ""
        if current != text:
            print(
                f"{OUTPUT.relative_to(REPO).as_posix()} is stale; run tools/contracts/gen_events.py",
                file=sys.stderr,
            )
            return 1
        print(f"{OUTPUT.relative_to(REPO).as_posix()} is up to date")
        return 0
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    with OUTPUT.open("w", encoding="utf-8", newline="\n") as fh:
        fh.write(text)
    print(f"wrote {OUTPUT.relative_to(REPO).as_posix()} ({len(json.loads(text)['events'])} event types)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
