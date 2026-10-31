"""Data compatibility checks between the Python and Rust implementations (L5).

Usage (from the repository root, with the venv python)::

    .venv/Scripts/python tools/compat_check.py [--rust PATH] [--python PATH]
        [--agent-dir DIR] [--only migrate|crossread|events|queue] [--keep]

Checks (each prints PASS or FAIL with the first differences; exit status 1 on any FAIL):

* ``migrate``   WG-3589 (a): every ``golden/db/rev-NNNN.db`` is migrated by ``init --no-workspace``
  with each implementation; the canonical dumps (``tools/contracts/dbdump.py``) must be equal.
* ``crossread`` WG-3589 (b): a small seed (the stack scenario, not the ~60 s ``demo seed``) written by one implementation, then the read commands (task
  list, stack list, analytics overview, doctor) with both, on copies of the home; outputs must
  agree after normalising the home path and volatile clock fields.
* ``events``    WG-3589 (c): ``doctor --verify-events`` with the other implementation on a seeded home.
* ``queue``     WG-3590: a stack scenario plus the demo seed's jobs drained by one Python and one
  Rust worker at once on one database; the domain-relevant rows must equal a Python-only run.

The whole script aborts with a FAIL if it passes 120 s of wall time.
Every run uses its own temporary ``WORKGRAPH_HOME``; the real data directory is never touched.
Defaults: ``--rust`` is ``$CARGO_TARGET_DIR/debug/workgraph.exe``, ``--python`` is the venv's
``workgraph``, ``--agent-dir`` is the directory holding ``workgraph-demo-agent`` (the Rust binary's).
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools" / "contracts"))
import dbdump  # noqa: E402

EXE = ".exe" if os.name == "nt" else ""
FAILURES: list[str] = []
BUDGET_SECONDS = 120
POOL = 12

# Fields that hold the wall clock or process identity; the same database read twice differs there.
VOLATILE = re.compile(r"(generated_at|_now$|^now$|elapsed|duration_ms|uptime|pid|checked_at)")


class Impl:
    def __init__(self, name: str, exe: Path, extra_path: list[Path]) -> None:
        self.name = name
        self.exe = exe
        self.extra_path = extra_path

    def env(self, home: Path) -> dict[str, str]:
        env = dict(os.environ)
        env["WORKGRAPH_HOME"] = str(home)
        env["PATH"] = os.pathsep.join([str(p) for p in self.extra_path] + [env.get("PATH", "")])
        env.pop("WORKGRAPH_WORKSPACE", None)
        return env

    def run(self, home: Path, *args: str, cwd: Path | None = None, check: bool = False) -> subprocess.CompletedProcess[str]:
        proc = subprocess.run(
            [str(self.exe), *args],
            env=self.env(home),
            cwd=str(cwd or home.parent),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=600,
        )
        if check and proc.returncode != 0:
            raise RuntimeError(f"{self.name} {' '.join(args)} exit {proc.returncode}: {proc.stderr[-800:]}{proc.stdout[-400:]}")
        return proc

    def popen(self, home: Path, *args: str, cwd: Path | None = None) -> subprocess.Popen[str]:
        return subprocess.Popen(
            [str(self.exe), *args],
            env=self.env(home),
            cwd=str(cwd or home.parent),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
        )


def migration_doc(database: Path) -> dict[str, Any]:
    """Canonical dump with the random ids and clock stamps of rows written by a data migration
    (``created_by`` is ``system:migration``) replaced, and those tables sorted by content."""
    doc = dbdump.canonical_document(database)
    for table in doc["tables"].values():
        rows = table["rows"]
        if not any(r.get("created_by") == "system:migration" for r in rows):
            continue
        for r in rows:
            if r.get("created_by") == "system:migration":
                for col in ("id", "created_at", "updated_at"):
                    if col in r:
                        r[col] = "<migration>"
        rows.sort(key=lambda r: json.dumps(r, sort_keys=True))
    return doc


def db_path(home: Path) -> Path:
    return home / "data" / "workgraph.db"


def report(name: str, ok: bool, detail: str = "") -> None:
    print(f"{'PASS' if ok else 'FAIL'}  {name}" + (f"\n{detail}" if detail and not ok else ""))
    if not ok:
        FAILURES.append(name)


def text_diff(a: Any, b: Any, limit: int = 40) -> str:
    sa = json.dumps(a, sort_keys=True, indent=1, ensure_ascii=False).splitlines()
    sb = json.dumps(b, sort_keys=True, indent=1, ensure_ascii=False).splitlines()
    return "\n".join(list(difflib.unified_diff(sa, sb, "python", "rust", lineterm="", n=1))[:limit])


# ------------------------------------------------------------------------------ normalisation


# Doctor checks that describe the runtime or machine, not the stored data.
ENVIRONMENT_CHECKS = ("python", "web frontend build", "llm provider")


def scrub(value: Any, homes: list[Path]) -> Any:
    if isinstance(value, list):
        value = [v for v in value if not (isinstance(v, dict) and str(v.get("check", "")).startswith(ENVIRONMENT_CHECKS))]
    if isinstance(value, dict):
        return {k: ("<volatile>" if VOLATILE.search(k) else scrub(v, homes)) for k, v in value.items()}
    if isinstance(value, list):
        return [scrub(v, homes) for v in value]
    if isinstance(value, str):
        for h in homes:
            for form in (str(h), str(h).replace("\\", "/"), str(h).replace("\\", "\\\\")):
                value = value.replace(form, "<home>")
        return value
    return value


def parse(out: str) -> Any:
    try:
        return json.loads(out)
    except ValueError:
        return out.strip()


# ------------------------------------------------------------------------------ (a) migrations


def check_migrate(py: Impl, rs: Impl, tmp: Path) -> None:
    fixtures = sorted((ROOT / "golden" / "db").glob("rev-*.db"))

    def one(job: tuple[Path, Impl]) -> Any:
        fixture, impl = job
        home = tmp / f"mig-{fixture.stem}-{impl.name}"
        (home / "data").mkdir(parents=True)
        shutil.copy(fixture, db_path(home))
        proc = impl.run(home, "init", "--no-workspace")
        if proc.returncode != 0:
            return {"error": proc.stderr[-500:] + proc.stdout[-300:]}
        return migration_doc(db_path(home))

    jobs = [(f, i) for f in fixtures for i in (py, rs)]
    with ThreadPoolExecutor(POOL) as pool:
        docs = list(pool.map(one, jobs))
    for n, fixture in enumerate(fixtures):
        p, r = docs[2 * n], docs[2 * n + 1]
        report(f"migrate {fixture.name}", p == r, text_diff(p, r))


# ------------------------------------------------------------------------------ (b) cross reads

READS = [
    ["--json", "task", "list"],
    ["--json", "stack", "list"],
    ["--json", "analytics", "overview"],
    ["--json", "doctor"],
]


def seed(impl: Impl, tmp: Path, label: str) -> Path:
    """A small seed: the stack scenario (workspace, repo, tasks, stack, queued run jobs) written by ``impl``."""
    home = tmp / f"seed-{label}"
    home.mkdir(parents=True)
    impl.pending_runs = setup_stack(impl, home, make_repo(tmp, f"seed-{label}"))  # type: ignore[attr-defined]
    return home


def copy_home(src: Path, dst: Path) -> Path:
    shutil.copytree(src, dst)
    return dst


def check_crossread(py: Impl, rs: Impl, tmp: Path, homes: dict[str, Path]) -> None:
    def one(job: tuple[Impl, list[str], Impl]) -> tuple[int, Any, Path]:
        writer, args, reader = job
        h = copy_home(homes[writer.name], tmp / f"read-{writer.name}-{reader.name}-{'_'.join(args[1:])}")
        proc = reader.run(h, *args)
        return proc.returncode, parse(proc.stdout), h

    jobs = [(w, a, r) for w in (py, rs) for a in READS for r in (py, rs)]
    with ThreadPoolExecutor(POOL) as pool:
        outs = list(pool.map(one, jobs))
    for n in range(0, len(jobs), 2):
        writer, args, _ = jobs[n]
        (c1, v1, h1), (c2, v2, h2) = outs[n], outs[n + 1]
        used = [h1, h2]
        ok = (c1, scrub(v1, used)) == (c2, scrub(v2, used))
        report(f"{writer.name}-writes, both read: {' '.join(args[1:])}", ok, text_diff(scrub(v1, used), scrub(v2, used)))


# ------------------------------------------------------------------------------ (c) events


def check_events(py: Impl, rs: Impl, tmp: Path, homes: dict[str, Path]) -> None:
    def one(job: tuple[Impl, Impl, str]) -> subprocess.CompletedProcess[str]:
        writer, reader, tag = job
        h = copy_home(homes[writer.name], tmp / f"events-{writer.name}-{tag}")
        return reader.run(h, "doctor", "--verify-events")

    jobs = [(py, rs, "x"), (rs, py, "x"), (py, py, "self"), (rs, rs, "self")]
    with ThreadPoolExecutor(POOL) as pool:
        procs = list(pool.map(one, jobs))
    for (writer, reader, _), proc in zip(jobs, procs):
        report(f"doctor --verify-events: {writer.name}-written, {reader.name} verifies",
               proc.returncode == 0, f"exit {proc.returncode} " + proc.stdout[-1200:] + proc.stderr[-400:])


# ------------------------------------------------------------------------------ (WG-3590) queue


def git(cwd: Path, *args: str) -> None:
    subprocess.run(["git", *args], cwd=str(cwd), check=True, capture_output=True,
                   env={**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
                        "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"})


def make_repo(tmp: Path, label: str) -> Path:
    repo = tmp / f"svc-{label}"
    repo.mkdir(parents=True)
    git(repo, "init", "-b", "main")
    (repo / "base.txt").write_text("base\n")
    git(repo, "add", "-A")
    git(repo, "commit", "-m", "initial")
    return repo


def setup_stack(impl: Impl, home: Path, repo: Path) -> list[str]:
    """Build the stack scenario with ``impl`` and return the ``run start`` commands still to issue."""
    c = lambda *a: impl.run(home, *a, check=True)  # noqa: E731
    c("init")
    for key, val in (("execution.enabled", "true"), ("llm.auto_embed", "false"),
                     ("llm.auto_classify", "false"), ("stacks.auto_accept", "true")):
        c("config", "set", key, val)
    for name, fname in (("fake-a", "a.py"), ("fake-b", "b.py")):
        c("agent", "add-command", name, "workgraph-demo-agent", "--arg=--mode", "--arg=success",
          "--arg=--file", f"--arg={fname}", "--arg={manifest}", "--timeout", "120", "--supports-usage")
    c("repo", "register", str(repo))
    c("task", "create", "Parent work", "--repo", repo.name)
    check = '{"name":"%s","kind":"%s","command":["python","-c","print(\'1 passed\')"]%s}'
    c("verify", "config", "gate", "--scope-type", "repository", "--scope", repo.name,
      "--check", check % ("types", "typecheck", ',"tier":"fast"'), "--check", check % ("suite", "test", ""))
    c("task", "create", "First change", "--parent", "WG-1", "--repo", repo.name, "--ready")
    c("task", "create", "Second change", "--parent", "WG-1", "--repo", repo.name, "--ready")
    c("stack", "create", "WG-1")
    return [["run", "start", "WG-2", "--agent", "fake-a", "--no-wait"],
            ["run", "start", "WG-3", "--agent", "fake-b", "--no-wait"]]


JOB_STATE_SQL = "SELECT kind, status FROM jobs ORDER BY kind, status"


def pending(home: Path) -> int:
    conn = sqlite3.connect(str(db_path(home)), timeout=30)
    try:
        return conn.execute("SELECT count(*) FROM jobs WHERE status IN ('queued','pending','running','leased')").fetchone()[0]
    finally:
        conn.close()


# Job kinds the Rust worker has handlers for. A Rust worker without --kind claims every kind and
# dead-letters the unported ones ("handler not ported"), which is itself a finding reported by this
# check's author; the mixed run restricts it so the queue can be shared.
RUST_WIRED_KINDS = ("git_sync", "agent_run", "verify", "stack_integrate", "stack_verify")


def drain(home: Path, workers: list[Impl], tmp: Path, label: str) -> str:
    procs = []
    for w in workers:
        kinds = [x for k in RUST_WIRED_KINDS for x in ("--kind", k)] if w.name == "rust" and len(workers) > 1 else []
        procs.append(w.popen(home, "worker", "run", "--max-idle", "2", "--concurrency", "2", *kinds))
    deadline = time.time() + 70
    logs = []
    for w, p in zip(workers, procs):
        try:
            out, _ = p.communicate(timeout=max(1, deadline - time.time()))
        except subprocess.TimeoutExpired:
            p.kill()
            out, _ = p.communicate()
            out += "\n[killed on timeout]"
        logs.append(f"--- {w.name} worker exit {p.returncode} {out[-300:]}")
    # Jobs queued after a worker went idle (follow-ups of the other worker's jobs) are drained by Python.
    for _ in range(6):
        if pending(home) == 0:
            break
        workers[0].run(home, "worker", "run", "--once")
        time.sleep(1.0)  # a follow-up job may be scheduled a moment ahead
    return " ".join(logs)


# Domain columns per table; ids, timestamps, paths and worker identities are excluded.
# git.commits_attributed is left out of the event counts: how the attributed commits split across
# analysis jobs depends on whether a job ran before or after another commit landed (timing).
DOMAIN_SQL = {
    "tasks": "SELECT key, title, status, parent_id IS NOT NULL AS has_parent FROM tasks ORDER BY key",
    "stacks": "SELECT status FROM stacks ORDER BY id",
    "jobs": "SELECT kind, status, attempts FROM jobs ORDER BY kind, status, attempts",
    "runs": "SELECT status FROM agent_runs ORDER BY status",
    "attempts": "SELECT status FROM attempts ORDER BY status",
    "event_types": "SELECT event_type, count(*) FROM events WHERE event_type != 'git.commits_attributed' GROUP BY event_type ORDER BY event_type",
}


def domain_rows(home: Path) -> dict[str, Any]:
    conn = sqlite3.connect(str(db_path(home)), timeout=30)
    out: dict[str, Any] = {}
    try:
        for name, sql in DOMAIN_SQL.items():
            try:
                out[name] = [list(r) for r in conn.execute(sql)]
            except sqlite3.Error as exc:  # schema name drift: report, do not hide
                out[name] = f"sql error: {exc}"
    finally:
        conn.close()
    return out


def check_queue(py: Impl, rs: Impl, tmp: Path, src: Path) -> None:
    """Start the scenario's runs (queueing jobs) in a copy of the Python-written seed, copy that, and
    drain each copy: Python alone, then a Python and a Rust worker together."""
    base_home = copy_home(src, tmp / "queue-src")
    for cmd in py.pending_runs:  # type: ignore[attr-defined]
        py.run(base_home, *cmd, check=True)
    repo = next(tmp.glob("svc-seed-python"))
    pristine = tmp / "svc-pristine"
    shutil.copytree(repo, pristine)

    def one(label: str, workers: list[Impl]) -> tuple[dict[str, Any], str]:
        # Same attempt ids, hence same branch names: each run starts from a pristine repository.
        shutil.rmtree(repo, onerror=lambda f, p, e: (os.chmod(p, 0o700), f(p)))
        shutil.copytree(pristine, repo)
        home = copy_home(base_home, tmp / f"queue-{label}")
        log = f"[{label}] pending before: {pending(home)} " + drain(home, workers, tmp, label)
        log += f" pending after: {pending(home)}"
        return domain_rows(home), log

    base, log1 = one("python-only", [py])
    mixed, log2 = one("mixed", [py, rs])
    report("mixed queue: Python+Rust workers equal Python-only run", base == mixed,
           text_diff(base, mixed) + " | " + log1 + " | " + log2)


# ------------------------------------------------------------------------------ main


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    scripts = Path(sys.executable).parent
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    ap.add_argument("--python", type=Path, default=scripts / f"workgraph{EXE}")
    ap.add_argument("--rust", type=Path, default=target / "debug" / f"workgraph{EXE}")
    ap.add_argument("--agent-dir", type=Path, default=None)
    ap.add_argument("--only", choices=["migrate", "crossread", "events", "queue"], action="append")
    ap.add_argument("--keep", action="store_true", help="keep the temporary directory")
    args = ap.parse_args()
    want = set(args.only or ["migrate", "crossread", "events", "queue"])

    agent_dir = args.agent_dir or args.rust.parent
    py = Impl("python", args.python, [agent_dir])
    rs = Impl("rust", args.rust, [agent_dir])
    tmp = Path(tempfile.mkdtemp(prefix="wg-compat-"))
    print(f"working directory: {tmp}")
    start = time.time()

    def watchdog() -> None:
        print(f"FAIL  time budget: exceeded {BUDGET_SECONDS} s", flush=True)
        os._exit(1)

    timer = threading.Timer(BUDGET_SECONDS, watchdog)
    timer.daemon = True
    timer.start()
    try:
        def later() -> None:
            with ThreadPoolExecutor(2) as pool:
                hp, hr = pool.map(lambda i: seed(i, tmp, i.name), (py, rs))
            homes = {py.name: hp, rs.name: hr}
            with ThreadPoolExecutor(3) as pool:
                futs = []
                if "queue" in want:
                    futs.append(pool.submit(check_queue, py, rs, tmp, hp))
                if "crossread" in want:
                    futs.append(pool.submit(check_crossread, py, rs, tmp, homes))
                if "events" in want:
                    futs.append(pool.submit(check_events, py, rs, tmp, homes))
                for f in futs:
                    f.result()

        stages = []
        if "migrate" in want:
            stages.append(lambda: check_migrate(py, rs, tmp))
        if want & {"crossread", "events", "queue"}:
            stages.append(later)
        with ThreadPoolExecutor(len(stages) or 1) as pool:
            for f in [pool.submit(st) for st in stages]:
                try:
                    f.result()
                except Exception as exc:  # a crashed stage is a failure, not a silent skip
                    report(f"stage crashed: {type(exc).__name__}", False, str(exc)[-800:])
    finally:
        timer.cancel()
        if not args.keep and not FAILURES:
            shutil.rmtree(tmp, ignore_errors=True)
    elapsed = time.time() - start
    print(f"wall time {elapsed:.1f} s (budget {BUDGET_SECONDS} s)")
    print(f"{len(FAILURES)} failure(s)" if FAILURES else "all checks passed")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())
