"""Stack executor parity: seed a stack on real repositories, run the Python executor, dump the result.

The Rust executor is checked against this one (``crates/wg-stacks-feature/tests/sh25_stack_exec.rs``): both
start from the same seeded home and repositories, run the same jobs, and the normalised dumps of
their rows, events, files and git refs must be equal.

Subcommands (every ``live`` is the directory the home and repositories sit in; the absolute paths
stored in the database are those of ``live``, so a seed is restored to the same place before each
run):

* ``seed LIVE SEED SCENARIO``  build a scenario in LIVE with Python, stopping before any stack job
  runs, and copy LIVE to SEED;
* ``restore SEED LIVE``        replace LIVE with a copy of SEED;
* ``run LIVE SCENARIO``        run the scenario's flow with the Python executor;
* ``dump LIVE OUT``            write the normalised dump of LIVE to OUT (JSON);
* ``compare A B``              print the differences between two dumps; exit 1 when there are any;
* ``flow SCENARIO``            print the scenario's flow as JSON (the Rust side replays it).
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
from pathlib import Path
from typing import Any

GIT_ENV = {
    "GIT_AUTHOR_NAME": "Test Author",
    "GIT_AUTHOR_EMAIL": "author@example.com",
    "GIT_COMMITTER_NAME": "Test Committer",
    "GIT_COMMITTER_EMAIL": "committer@example.com",
    "GIT_CONFIG_NOSYSTEM": "1",
}

STACK_KINDS = ["stack_integrate", "stack_verify"]

# What each scenario builds and then does. ``checks`` are the checks of the repository gate;
# ``children`` are the stack's children in creation order (``file`` is what its agent writes).
PASSING = "print('1 passed')"
SCENARIOS: dict[str, dict[str, Any]] = {
    "verify_pass": {
        "auto_accept": False,
        "children": [{"file": "a.py"}, {"file": "b.py"}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {"name": "suite", "tier": "full", "code": PASSING},
        ],
        "flow": [{"drain": True}],
    },
    "land": {
        "auto_accept": True,
        "children": [{"file": "a.py"}, {"file": "b.py"}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {"name": "suite", "tier": "full", "code": PASSING},
        ],
        "flow": [{"drain": True}],
    },
    "no_full_checks": {
        "auto_accept": False,
        "children": [{"file": "a.py"}],
        "checks": [{"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING}],
        "flow": [{"drain": True}],
    },
    "bisect": {
        "auto_accept": False,
        "children": [{"file": "a.py"}, {"file": "bad.txt"}, {"file": "c.py"}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {
                "name": "no-bad",
                "tier": "full",
                "code": (
                    "import os, sys\n"
                    "if os.path.exists('bad.txt'):\n"
                    "    print('FAILED tests/test_bad.py::test_exists - AssertionError')\n"
                    "    sys.exit(1)\n"
                    "print('1 passed')\n"
                ),
            },
        ],
        "flow": [{"drain": True}],
    },
    "regression": {
        "auto_accept": False,
        "children": [{"file": "a.py"}, {"file": "bad.txt"}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {
                "name": "suite",
                "tier": "full",
                "baseline": "regression",
                "code": (
                    "import os, sys\n"
                    "print('FAILED tests/test_old.py::test_known - AssertionError')\n"
                    "if os.path.exists('bad.txt'):\n"
                    "    print('FAILED tests/test_bad.py::test_exists - AssertionError')\n"
                    "sys.exit(1)\n"
                ),
            },
        ],
        "flow": [{"drain": True}],
    },
    "tolerated": {
        "auto_accept": False,
        "children": [{"file": "a.py"}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {
                "name": "suite",
                "tier": "full",
                "baseline": "regression",
                "code": (
                    "import sys\n"
                    "print('FAILED tests/test_old.py::test_known - AssertionError')\n"
                    "sys.exit(1)\n"
                ),
            },
        ],
        "flow": [{"drain": True}],
    },
    "base_fails": {
        "auto_accept": False,
        "children": [{"file": "a.py"}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {
                "name": "suite",
                "tier": "full",
                "code": (
                    "import sys\n"
                    "print('FAILED tests/test_old.py::test_known - AssertionError')\n"
                    "sys.exit(1)\n"
                ),
            },
        ],
        "flow": [{"drain": True}],
    },
    "errored": {
        "auto_accept": False,
        "children": [{"file": "a.py"}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {
                "name": "suite",
                "tier": "full",
                "error_exit_codes": [99],
                "code": "import sys\nprint('could not run')\nsys.exit(99)\n",
            },
        ],
        "flow": [{"drain": True}],
    },
    "conflict": {
        "auto_accept": False,
        "shared": True,
        "children": [{"file": "a.py"}, {"file": "b.py"}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {"name": "suite", "tier": "full", "code": PASSING},
        ],
        "flow": [{"drain": True}],
    },
    "dependent": {
        "auto_accept": False,
        "children": [{"file": "a.py"}, {"file": "b.py", "depends_on": [0]}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {"name": "suite", "tier": "full", "code": PASSING},
        ],
        "flow": [{"drain": True}],
    },
    "manual_eject": {
        "auto_accept": False,
        "children": [{"file": "a.py"}, {"file": "b.py"}, {"file": "c.py", "depends_on": [1]}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {"name": "suite", "tier": "full", "code": PASSING},
        ],
        "flow": [
            {"drain": True},
            {"eject": 1, "reason": "b breaks the release build"},
            {"drain": True},
        ],
    },
    "stale": {
        "auto_accept": False,
        "children": [{"file": "a.py"}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {"name": "suite", "tier": "full", "code": PASSING},
        ],
        "main_moves": True,
        "flow": [{"drain": True}],
    },
    "stale_land": {
        "auto_accept": True,
        "children": [{"file": "a.py"}, {"file": "b.py"}],
        "checks": [
            {"name": "types", "kind": "typecheck", "tier": "fast", "code": PASSING},
            {"name": "suite", "tier": "full", "code": PASSING},
        ],
        "main_moves": True,
        "flow": [{"drain": True}],
    },
}


# ---------------------------------------------------------------------------- files


def _writable(function, path, _error) -> None:
    """Read-only object files cannot be deleted on Windows."""
    Path(path).chmod(0o700)
    function(path)


def remove_tree(path: Path) -> None:
    if path.exists():
        shutil.rmtree(path, onexc=_writable)


# ---------------------------------------------------------------------------- git helpers


def git(path: Path, *args: str, env: dict[str, str] | None = None) -> str:
    full_env = {**os.environ, **GIT_ENV, **(env or {})}
    full_env["GIT_CONFIG_GLOBAL"] = str(path.parent / ".gitconfig-parity")
    proc = subprocess.run(
        ["git", "-C", str(path), *args], capture_output=True, text=True, env=full_env, check=False
    )
    if proc.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr}")
    return proc.stdout.strip()


def commit(path: Path, message: str, files: dict[str, str], tick: int) -> None:
    for rel, content in files.items():
        target = path / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8", newline="\n")
    git(path, "add", "-A")
    stamp = f"2026-01-01T00:{tick // 60:02d}:{tick % 60:02d}+00:00"
    git(
        path,
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        message,
        env={"GIT_AUTHOR_DATE": stamp, "GIT_COMMITTER_DATE": stamp},
    )


# ---------------------------------------------------------------------------- build


def open_app(live: Path):
    from workgraph.app import Workgraph
    from workgraph.config.paths import AppPaths

    os.environ["WORKGRAPH_HOME"] = str(live / "home")
    return Workgraph(AppPaths.resolve(live / "home"))


def check_dict(spec: dict[str, Any]) -> dict[str, Any]:
    check: dict[str, Any] = {
        "name": spec["name"],
        "kind": spec.get("kind", "test"),
        "command": [sys.executable, "-c", spec["code"]],
        "tier": spec["tier"],
    }
    for key in ("baseline", "error_exit_codes"):
        if key in spec:
            check[key] = spec[key]
    return check


def drain(app, kinds: list[str] | None, timeout: float = 180) -> None:
    from workgraph.jobs.worker import Worker

    worker = Worker(app, concurrency=2, poll_interval=0.05, maintenance_interval=0.2, kinds=kinds)
    worker.run_until_idle(timeout=timeout)


def non_stack_kinds() -> list[str]:
    from workgraph.jobs.handlers import HANDLERS

    return [kind for kind in HANDLERS if kind not in STACK_KINDS]


def seed(live: Path, seed_dir: Path, name: str) -> None:
    from workgraph.config.settings import CommandAgentConfig
    from workgraph.core.context import RequestContext
    from workgraph.core.models.schemas import RepositoryRegister, TaskCreate, WorkspaceCreate
    from workgraph.demo import scripted_agent

    spec = SCENARIOS[name]
    remove_tree(live)
    live.mkdir(parents=True)
    repo = live / "repos" / "svc"
    repo.mkdir(parents=True)
    git(repo, "init", "-q", "-b", "main")
    git(repo, "config", "user.name", "Test Author")
    git(repo, "config", "user.email", "author@example.com")
    git(repo, "config", "core.autocrlf", "false")
    commit(repo, "initial commit", {"README.md": "# svc\n"}, 1)
    if spec.get("shared"):
        plan = {"file": "shared.py", "content": "VALUE = '{task}'\n"}
        commit(repo, "add demo plan", {"workgraph-demo.json": json.dumps(plan) + "\n"}, 2)

    app = open_app(live)
    human = RequestContext.human("tester")
    config = app.config
    config.llm.auto_classify = False
    config.llm.auto_embed = False
    config.execution.enabled = True
    config.stacks.auto_accept = spec["auto_accept"]
    files = sorted({child["file"] for child in spec["children"]})
    for file in files:
        config.agents[f"fake-{file}"] = CommandAgentConfig(
            executable=sys.executable,
            args=[scripted_agent.__file__, "--mode", "success", "--file", file, "{manifest}"],
            timeout_seconds=120,
            supports_usage=True,
        )
    config.execution.default_agent = f"fake-{files[0]}"
    app.config_store.save_global(config)
    ws = app.workspaces.create(
        human, WorkspaceCreate(slug="main", name="Main", key_prefix="WG"), make_default=True
    ).slug
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo)))
    parent = app.tasks.create(human, ws, TaskCreate(title="Parent work", repositories=["svc"]))
    app.verification.create_config(
        human,
        ws,
        scope_type="repository",
        scope="svc",
        name="gate",
        checks=[check_dict(c) for c in spec["checks"]],
    )
    children = []
    for index, child in enumerate(spec["children"]):
        created = app.tasks.create(
            human,
            ws,
            TaskCreate(
                title=f"Change {index + 1}",
                parent=parent.key,
                repositories=["svc"],
                status="ready",
                depends_on=[children[i].key for i in child.get("depends_on", [])],
            ),
        )
        children.append(created)
    app.stacks.create(human, ws, parent.key)
    for index, child in enumerate(spec["children"]):
        attempt = app.runs.start(human, ws, children[index].key, agent=f"fake-{child['file']}")
        drain(app, non_stack_kinds())
        assert app.attempts.get(human, attempt.id).status == "succeeded"
    if spec.get("main_moves"):
        commit(repo, "unrelated change on main", {"other.txt": "other\n"}, 30)
    app.close()
    remove_tree(seed_dir)
    shutil.copytree(live, seed_dir, symlinks=True)


def restore(seed_dir: Path, live: Path) -> None:
    remove_tree(live)
    shutil.copytree(seed_dir, live, symlinks=True)


def run_flow(live: Path, name: str) -> None:
    from workgraph.core.context import RequestContext

    spec = SCENARIOS[name]
    app = open_app(live)
    human = RequestContext.human("tester")
    parent = "WG-1"
    for step in spec["flow"]:
        if step.get("drain"):
            drain(app, STACK_KINDS)
        if "eject" in step:
            child = f"WG-{step['eject'] + 2}"
            app.stack_verification.eject(human, "main", parent, child, step["reason"])
    app.close()


# ---------------------------------------------------------------------------- dump

UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
SHA = re.compile(r"\b[0-9a-f]{40}\b")
TABLES = [
    "tasks",
    "task_comments",
    "task_dependencies",
    "stacks",
    "stack_repositories",
    "stack_items",
    "jobs",
    "verification_runs",
    "verification_check_results",
    "task_acceptances",
    "revision_manifests",
    "managed_worktrees",
    "artifacts",
    "events",
]
JSON_COLUMNS = {
    "data",
    "detail",
    "payload",
    "result",
    "entries",
    "config_snapshot",
    "failure_signatures",
    "resource_keys",
    "land_request",
    "metadata",
    "metadata_",
    "acceptance_criteria",
    "estimated_effort",
}
# Columns that hold a moment, a duration or a worker: their presence is compared, not their value.
VOLATILE = re.compile(
    r"(_at|^at)$|^(duration_ms|lease_owner|seq|updated_at|created_at|occurred_at)$|^(started_at|finished_at)$"
)
SKIP_COLUMNS = {
    "events": {"seq"},
    "jobs": {"lease_owner", "attempts", "last_error"},
    "task_acceptances": set(),
}


ARTIFACTS_DIR: Path | None = None


class Names:
    """Stable tokens for ids and commits, by order of first appearance."""

    def __init__(self) -> None:
        self.tokens: dict[str, str] = {}

    def token(self, value: str, kind: str) -> str:
        if value not in self.tokens:
            self.tokens[value] = f"<{kind}{len(self.tokens)}>"
        return self.tokens[value]

    def text(self, value: str, live: str) -> str:
        value = value.replace(live, "<LIVE>").replace(live.replace("\\", "/"), "<LIVE>")
        value = value.replace("\\\\", "/").replace("\\", "/")
        value = UUID.sub(lambda m: self.token(m.group(0), "id"), value)
        return SHA.sub(lambda m: self.token(m.group(0), "sha"), value)


def shape(value: Any, live: str) -> Any:
    """The structure of a value with ids, commits and moments masked, for ordering rows."""
    if isinstance(value, dict):
        return {k: shape(v, live) for k, v in sorted(value.items())}
    if isinstance(value, list):
        return [shape(v, live) for v in value]
    if isinstance(value, str):
        masked = value.replace(live, "<LIVE>").replace(live.replace("\\", "/"), "<LIVE>")
        masked = masked.replace("\\\\", "/").replace("\\", "/")
        return SHA.sub("<sha>", UUID.sub("<id>", masked))
    return value


def decode(column: str, value: Any) -> Any:
    if column in JSON_COLUMNS and isinstance(value, str):
        try:
            return json.loads(value)
        except json.JSONDecodeError:
            return value
    return value


def rows_of(db: sqlite3.Connection, table: str, live: str) -> list[dict[str, Any]]:
    db.row_factory = sqlite3.Row
    skip = SKIP_COLUMNS.get(table, set())
    order = " ORDER BY seq" if table == "events" else ""
    if table == "jobs":
        # The maintenance pass queues housekeeping jobs of its own; only the stack ones matter.
        order = " WHERE kind IN ('stack_integrate', 'stack_verify', 'compute_change_surface')"
    rows = []
    for raw in db.execute(f"SELECT * FROM {table}{order}"):
        row = {}
        for column in raw:
            if column in skip:
                continue
            value = decode(column, raw[column])
            if VOLATILE.search(column) and value is not None:
                value = "<moment>"
            row[column] = value
        if table == "artifacts" and ARTIFACTS_DIR is not None:
            # Python writes logs in text mode, so Windows gives them CRLF; compare the lines.
            data = (ARTIFACTS_DIR / row["path"]).read_bytes().replace(b"\r\n", b"\n")
            row["size_bytes"] = len(data)
            row["sha256"] = hashlib.sha256(data).hexdigest()
        rows.append(row)
    if table != "events":
        rows.sort(key=lambda r: json.dumps(shape(r, live), sort_keys=True))
    return rows


def mask(value: Any, names: Names, live: str) -> Any:
    if isinstance(value, dict):
        return {k: mask(v, names, live) for k, v in value.items()}
    if isinstance(value, list):
        return [mask(v, names, live) for v in value]
    if isinstance(value, str):
        return names.text(value, live)
    return value


def tree_of(root: Path, names: Names, live: str) -> dict[str, Any]:
    """The files below ``root`` (relative names with ids masked), manifests and logs by content."""
    found: dict[str, Any] = {}
    if not root.exists():
        return found
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        rel = names.text(path.relative_to(root).as_posix(), live)
        text = path.read_text(encoding="utf-8", errors="replace")
        if path.suffix == ".json":
            try:
                found[rel] = mask(json.loads(text), names, live)
                continue
            except json.JSONDecodeError:
                pass
        found[rel] = names.text(text, live)
    return found


def git_state(repo: Path, names: Names, live: str) -> dict[str, Any]:
    refs = git(repo, "for-each-ref", "--format=%(refname) %(objectname)").splitlines()
    state: dict[str, Any] = {"refs": {}, "worktrees": []}
    for line in refs:
        ref, sha = line.split(" ")
        history = git(repo, "log", "--format=%s|%P", sha).splitlines()
        state["refs"][ref] = {
            "head": names.token(sha, "sha"),
            "log": [
                [subject, len(parents.split())]
                for subject, parents in (entry.rsplit("|", 1) for entry in history)
            ],
        }
    for block in git(repo, "worktree", "list", "--porcelain").split("\n\n"):
        entry = {}
        for line in block.splitlines():
            key, _, value = line.partition(" ")
            entry[key] = names.text(value, live) if key != "HEAD" else names.token(value, "sha")
        if entry:
            state["worktrees"].append(entry)
    state["worktrees"].sort(key=lambda e: e.get("worktree", ""))
    return state


def dump(live: Path, out: Path) -> None:
    from workgraph.config.paths import AppPaths

    global ARTIFACTS_DIR  # noqa: PLW0603 - set once per dump
    live_text = str(live.resolve())
    paths = AppPaths.resolve(live / "home")
    ARTIFACTS_DIR = paths.artifacts_dir
    db = sqlite3.connect(str(paths.database_file))
    names = Names()
    tables = {table: rows_of(db, table, live_text) for table in TABLES}
    db.close()
    document: dict[str, Any] = {}
    # Events first: they name nearly every id in the order things happened.
    for table in ["events", *[t for t in TABLES if t != "events"]]:
        document[table] = [mask(row, names, live_text) for row in tables[table]]
    document["artifact_files"] = tree_of(paths.artifacts_dir, names, live_text)
    document["run_files"] = tree_of(paths.runs_dir, names, live_text)
    document["git"] = git_state(live / "repos" / "svc", names, live_text)
    # Python and Rust order the keys of a JSON text differently; the dump is compared as data.
    out.write_text(json.dumps(document, indent=1, sort_keys=True), encoding="utf-8")


def compare(left: Path, right: Path) -> int:
    a = json.loads(left.read_text(encoding="utf-8"))
    b = json.loads(right.read_text(encoding="utf-8"))
    problems: list[str] = []

    def walk(x: Any, y: Any, where: str) -> None:
        if type(x) is not type(y):
            problems.append(f"{where}: {x!r} != {y!r}")
        elif isinstance(x, dict):
            for key in sorted(set(x) | set(y)):
                if key not in x:
                    problems.append(f"{where}.{key}: missing on the left; right has {y[key]!r}")
                elif key not in y:
                    problems.append(f"{where}.{key}: missing on the right; left has {x[key]!r}")
                else:
                    walk(x[key], y[key], f"{where}.{key}")
        elif isinstance(x, list):
            if len(x) != len(y):
                problems.append(f"{where}: {len(x)} entries on the left, {len(y)} on the right")
            for index, (p, q) in enumerate(zip(x, y, strict=False)):
                walk(p, q, f"{where}[{index}]")
        elif x != y:
            problems.append(f"{where}: {x!r} != {y!r}")

    walk(a, b, "")
    for problem in problems[:80]:
        print(problem)
    if len(problems) > 80:
        print(f"... and {len(problems) - 80} more")
    return 1 if problems else 0


def main(argv: list[str]) -> int:
    command, *rest = argv
    if command == "seed":
        seed(Path(rest[0]), Path(rest[1]), rest[2])
    elif command == "restore":
        restore(Path(rest[0]), Path(rest[1]))
    elif command == "run":
        run_flow(Path(rest[0]), rest[1])
    elif command == "dump":
        dump(Path(rest[0]), Path(rest[1]))
    elif command == "compare":
        return compare(Path(rest[0]), Path(rest[1]))
    elif command == "flow":
        print(json.dumps({"flow": SCENARIOS[rest[0]]["flow"], "scenarios": sorted(SCENARIOS)}))
    elif command == "scenarios":
        print(json.dumps(sorted(SCENARIOS)))
    else:
        print(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
