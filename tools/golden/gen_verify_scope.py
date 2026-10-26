"""Golden vectors for check selection (``verify``: applicable checks and tiers).

Drives ``workgraph.verification.configs`` (``applicable_checks``, ``checks_for_tier``) through a
temporary SQLite database seeded with fixed ids, and writes the observed selection to
``golden/verify/scope.json``. Each case lists the stored configurations (with their ids, in
insertion order) and the task's project and repository links as inputs, and the checks Python
selects, in order, for each tier.

Run with ``uv run python tools/golden/gen_verify_scope.py``. Output is deterministic: fixed ids,
sorted keys, two-space indent, LF line endings, UTF-8.
"""

from __future__ import annotations

import copy
import json
import shutil
import tempfile
from pathlib import Path
from typing import Any

from workgraph.persistence.db import Database
from workgraph.persistence.migrate import upgrade
from workgraph.persistence.orm import (
    Project,
    Repository,
    Task,
    TaskProject,
    TaskRepository,
    VerificationConfig,
    Workspace,
)
from workgraph.verification.configs import applicable_checks, checks_for_tier

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "golden" / "verify" / "scope.json"

WS = "00000000-0000-4000-8000-000000000001"
OTHER_WS = "00000000-0000-4000-8000-000000000002"
TASK = "00000000-0000-4000-8000-0000000000a1"
OTHER_TASK = "00000000-0000-4000-8000-0000000000a2"
PROJECT = "00000000-0000-4000-8000-0000000000b1"
OTHER_PROJECT = "00000000-0000-4000-8000-0000000000b2"
REPO = "00000000-0000-4000-8000-0000000000c1"
OTHER_REPO = "00000000-0000-4000-8000-0000000000c2"

LINT = {"name": "lint", "command": ["ruff", "check"], "tier": "fast"}
UNIT = {"name": "unit", "command": ["pytest", "-q"]}
TYPES = {"name": "types", "command": ["mypy", "src"], "tier": "fast", "parser": "generic"}
ALL = {"name": "all", "command": ["pytest"], "tier": "full", "required": False}
BASE_ONLY = {"name": "base", "command": ["make", "test"], "repository": "other", "baseline": "regression"}


def _config(
    name: str,
    scope_type: str,
    scope_id: str,
    checks: list[dict[str, Any]],
    *,
    workspace: str = WS,
    enabled: bool = True,
) -> dict[str, Any]:
    return {
        "name": name,
        "scope_type": scope_type,
        "scope_id": scope_id,
        "workspace": workspace,
        "checks": checks,
        "enabled": enabled,
    }


CASES: list[dict[str, Any]] = [
    {
        "id": "scopes-and-order",
        "task_projects": [PROJECT],
        "task_repositories": [REPO],
        "configs": [
            _config("ci", "workspace", WS, [UNIT, LINT]),
            _config("extra", "task", TASK, [ALL]),
            _config("lint", "project", PROJECT, [TYPES]),
            _config(
                "types",
                "repository",
                REPO,
                [{"name": "repo-check", "command": ["cargo", "test"], "tier": "fast"}],
            ),
            _config("other", "workspace", OTHER_WS, [UNIT], workspace=OTHER_WS),
            _config("off", "workspace", WS, [UNIT], enabled=False),
            _config("elsewhere", "project", OTHER_PROJECT, [UNIT]),
            _config("other-task", "task", OTHER_TASK, [UNIT]),
            _config("other-repo", "repository", OTHER_REPO, [UNIT]),
        ],
    },
    {
        "id": "repository-scope-keeps-explicit-repository",
        "task_projects": [],
        "task_repositories": [REPO],
        "configs": [
            _config("base", "repository", REPO, [BASE_ONLY, {"name": "fallback", "command": ["pytest"]}]),
        ],
    },
    {
        "id": "no-links-only-workspace-scope",
        "task_projects": [],
        "task_repositories": [],
        "configs": [
            _config("ws", "workspace", WS, [TYPES]),
            _config("proj", "project", PROJECT, [LINT]),
        ],
    },
    {
        "id": "same-scope-type-orders-by-name",
        "task_projects": [PROJECT, OTHER_PROJECT],
        "task_repositories": [],
        "configs": [
            _config("zeta", "project", PROJECT, [UNIT]),
            _config("alpha", "project", OTHER_PROJECT, [LINT]),
        ],
    },
]


def _config_id(index: int) -> str:
    return f"00000000-0000-4000-8000-{0xD000 + index:012x}"


def _seed(session: Any, case: dict[str, Any]) -> None:
    session.add(Workspace(id=WS, slug="ws", name="ws", key_prefix="WG"))
    session.add(Workspace(id=OTHER_WS, slug="other", name="other", key_prefix="OT"))
    session.flush()
    session.add(Task(id=TASK, workspace_id=WS, number=1, key="WG-1", title="t", acceptance_criteria=[]))
    session.add(Task(id=OTHER_TASK, workspace_id=WS, number=2, key="WG-2", title="t", acceptance_criteria=[]))
    session.add(Project(id=PROJECT, workspace_id=WS, slug="p", name="p"))
    session.add(Project(id=OTHER_PROJECT, workspace_id=WS, slug="q", name="q"))
    session.add(Repository(id=REPO, workspace_id=WS, name="core"))
    session.add(Repository(id=OTHER_REPO, workspace_id=WS, name="other-repo"))
    session.flush()
    for project in case["task_projects"]:
        session.add(TaskProject(task_id=TASK, project_id=project))
    for repository in case["task_repositories"]:
        session.add(TaskRepository(task_id=TASK, repository_id=repository, access="read_write"))
    session.flush()
    for index, config in enumerate(case["configs"]):
        session.add(
            VerificationConfig(
                id=_config_id(index),
                workspace_id=config["workspace"],
                scope_type=config["scope_type"],
                scope_id=config["scope_id"],
                name=config["name"],
                checks=copy.deepcopy(config["checks"]),
                enabled=config["enabled"],
                approved_by="human:test",
            )
        )
    session.flush()


def _check_out(check: Any) -> dict[str, Any]:
    return {
        "config_id": check.config_id,
        "config_name": check.config_name,
        "scope_type": check.scope_type,
        "scope_id": check.scope_id,
        "name": check.spec.name,
        "tier": check.spec.tier,
        "repository": check.spec.repository,
        "command": list(check.spec.command),
    }


def run_case(case: dict[str, Any]) -> dict[str, Any]:
    tmp = Path(tempfile.mkdtemp(prefix="wg-gv-verify-"))
    db = Database(tmp / "workgraph.db")
    try:
        upgrade(db)
        with db.session(write=True) as session:
            _seed(session, case)
        with db.session() as session:
            task = session.get(Task, TASK)
            assert task is not None
            checks = applicable_checks(session, task)
            fast = checks_for_tier(checks, "fast")
            full = checks_for_tier(checks, "full")
            return {
                "id": case["id"],
                "task_projects": case["task_projects"],
                "task_repositories": case["task_repositories"],
                "configs": [
                    {"id": _config_id(index), **config} for index, config in enumerate(case["configs"])
                ],
                "expected": {
                    "all": [_check_out(c) for c in checks],
                    "fast": [_check_out(c) for c in fast],
                    "full": [_check_out(c) for c in full],
                },
            }
    finally:
        db.dispose()
        shutil.rmtree(tmp, ignore_errors=True)


def main() -> None:
    doc = {"task": TASK, "workspace": WS, "cases": [run_case(case) for case in CASES]}
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
