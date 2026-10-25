"""Golden vectors GV-15 (analytics).

Builds two workspaces in a temporary WORKGRAPH_HOME and writes, per database, the source rows, a
sampled set of metric queries (see ``sampled_queries``: every metric, every allowed dimension and
every period appears, but not the cross product) and the report outputs:

* ``demo`` - ``workgraph demo seed`` (real services, scripted agents, real verification commands);
* ``gen``  - generated from seed 1 through the services, with synthetic attempts.

Determinism: the clock is a counter (one tick per call, six hours apart), identifiers come from a seeded
generator, monotonic time is a counter, git commits use fixed dates and the demo runs one worker
thread. Every UUID in the output is replaced by a stable label (``task:KEY``, ``project:SLUG``,
``repository:NAME``, ``commit:SHA``, ``attempt:KEY#N``, ``<table>:<rowid>``), and rows are sorted by
their label-valued group keys, so two runs produce identical bytes.

Run: ``uv run python tools/golden/gen_analytics.py``
"""

from __future__ import annotations

import base64
import json
import os
import random
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from concurrent.futures import Future
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "src"
OUT = ROOT / "golden" / "analytics"
SEED = 15
EPOCH = datetime(2026, 1, 5, 9, 0, 0, tzinfo=UTC)
TICK = timedelta(hours=6)
GIT_DATE = "2026-01-05T09:00:00+0000"
MAX_FILE_BYTES = 2 * 1024 * 1024
UUID_RE = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")

sys.path.insert(0, str(SRC))

# ---------------------------------------------------------------- deterministic environment
_clock_lock = threading.Lock()
_clock_ticks = [0]
_id_rng = random.Random(SEED)  # noqa: S311 - reproducible test data, not security
_id_lock = threading.Lock()


_job_tick: list[int | None] = [None]


def _utcnow() -> datetime:
    # While an agent job runs, the clock stays at the tick it started on: the number of calls made while
    # a subprocess is being supervised depends on wall-clock polling and must not move the timeline.
    with _clock_lock:
        if _job_tick[0] is not None:
            return EPOCH + TICK * _job_tick[0]
        _clock_ticks[0] += 1
        return EPOCH + TICK * _clock_ticks[0]


def _monotonic() -> float:
    # Measured durations (and the wait loops, which end when their process ends) read this. A constant
    # makes every measured duration zero on every run; the wall-clock poll count of a subprocess is not
    # reproducible, so a counting clock would not be either.
    return 0.0


def _new_id() -> str:
    with _id_lock:
        return str(uuid.UUID(int=_id_rng.getrandbits(128), version=4))


from workgraph.core import clock  # noqa: E402

clock.utcnow = _utcnow  # must precede every other workgraph import (column defaults bind it)
clock.new_id = _new_id
time.monotonic = _monotonic  # type: ignore[assignment]

import duckdb  # noqa: E402

import workgraph.integrations.process as process_mod  # noqa: E402

_orig_base_environment = process_mod.base_environment
_PINNED_GIT_ENV = {
    "GIT_AUTHOR_DATE": GIT_DATE,
    "GIT_COMMITTER_DATE": GIT_DATE,
    "GIT_AUTHOR_NAME": "Golden Author",
    "GIT_AUTHOR_EMAIL": "author@example.com",
    "GIT_COMMITTER_NAME": "Golden Author",
    "GIT_COMMITTER_EMAIL": "author@example.com",
}


def _pinned_base_environment(extra: Any = None, *, inherit_all: bool = False) -> dict[str, str]:
    # Agent subprocesses get a whitelisted environment without the git date variables; commits they make
    # would otherwise carry the wall-clock time. Pin the dates (and identity) for every child process.
    env = _orig_base_environment(extra, inherit_all=inherit_all)
    env.update(_PINNED_GIT_ENV)
    return env


process_mod.base_environment = _pinned_base_environment
for _module in list(sys.modules.values()):
    if getattr(_module, "base_environment", None) is _orig_base_environment:
        _module.base_environment = _pinned_base_environment  # type: ignore[attr-defined]

_duckdb_connect = duckdb.connect


def _single_threaded_connect(*args: Any, **kwargs: Any) -> Any:
    # DuckDB's multi-threaded aggregation returns groups in an unspecified order; one thread fixes it.
    kwargs["config"] = {"threads": 1, **kwargs.get("config", {})}
    return _duckdb_connect(*args, **kwargs)


duckdb.connect = _single_threaded_connect  # type: ignore[assignment]
os.environ["GIT_AUTHOR_DATE"] = GIT_DATE
os.environ["GIT_COMMITTER_DATE"] = GIT_DATE
os.environ.pop("WORKGRAPH_HOME", None)

from workgraph.analytics.metrics import BASE_DIMENSIONS, METRICS, PERIODS  # noqa: E402
from workgraph.analytics.retrospective import generate_retrospective  # noqa: E402
from workgraph.analytics.service import AnalyticsFilters, AnalyticsQuery  # noqa: E402
from workgraph.app import Workgraph  # noqa: E402
from workgraph.config.paths import AppPaths  # noqa: E402
from workgraph.core.context import RequestContext  # noqa: E402
from workgraph.core.models.enums import AttemptStatus  # noqa: E402
from workgraph.core.models.schemas import (  # noqa: E402
    ClaimRequest,
    RelationCreate,
    RepositoryRegister,
    TaskCreate,
    WorkspaceCreate,
)
from workgraph.core.services import resolve  # noqa: E402
from workgraph.core.services.attempts import finish_attempt, new_attempt  # noqa: E402
from workgraph.demo.seed import seed_demo  # noqa: E402
from workgraph.jobs import worker as worker_mod  # noqa: E402
from workgraph.persistence.orm import Task  # noqa: E402

_orig_worker_init = worker_mod.Worker.__init__


def _single_worker_init(self: Any, app: Any, *args: Any, **kwargs: Any) -> None:
    kwargs["concurrency"] = 1
    # The default id embeds the host name, pid and a random suffix, and is stored on claimed jobs.
    kwargs.setdefault("worker_id", "golden-worker")
    _orig_worker_init(self, app, *args, **kwargs)


def _start_in_caller_thread(self: Any, job: Any) -> None:
    # Jobs run to completion on the calling thread, so the order of clock and id calls is fixed. The
    # heartbeat thread only wakes after a lease third (40 s by default), which these jobs never reach.
    future: Future[None] = Future()
    with _clock_lock:
        _job_tick[0] = _clock_ticks[0]
    try:
        self._execute(job, worker_mod.JobContext(job=job, worker_id=self.worker_id))
        future.set_result(None)
    except BaseException as exc:  # recorded like a pool future; _reap collects it
        future.set_exception(exc)
    finally:
        with _clock_lock:
            _job_tick[0] = None
    with self._lock:
        self._running[job.id] = future


worker_mod.Worker.__init__ = _single_worker_init  # type: ignore[method-assign]
worker_mod.Worker._start = _start_in_caller_thread  # type: ignore[method-assign]

HUMAN = RequestContext.human("golden")
REPO_NAMES = ("auth-service", "shared-sdk", "web-client", "docs-site")


# ---------------------------------------------------------------- helpers
def _git(cwd: Path, *args: str, when: str | None = None) -> str:
    env = dict(os.environ)
    if when is not None:
        env["GIT_AUTHOR_DATE"] = when
        env["GIT_COMMITTER_DATE"] = when
    env.update(
        GIT_AUTHOR_NAME="Golden Author",
        GIT_AUTHOR_EMAIL="author@example.com",
        GIT_COMMITTER_NAME="Golden Author",
        GIT_COMMITTER_EMAIL="author@example.com",
    )
    out = subprocess.run(["git", *args], cwd=cwd, env=env, check=True, capture_output=True, text=True)
    return out.stdout.strip()


def _init_repo(path: Path) -> Path:
    path.mkdir(parents=True)
    _git(path, "init", "-q", "-b", "main")
    _git(path, "config", "core.autocrlf", "false")
    return path


def _commit(path: Path, message: str, files: dict[str, str], when: str) -> str:
    for rel, text in files.items():
        target = path / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")
    _git(path, "add", "-A")
    _git(path, "commit", "-q", "--allow-empty", "-m", message, when=when)
    return _git(path, "rev-parse", "HEAD")


def _add_attempt(
    app: Workgraph,
    task_id: str,
    *,
    provider: str,
    model: str | None,
    status: str,
    duration_ms: int,
    queue_ms: int,
) -> None:
    with app.db.session(write=True) as session:
        task = resolve.task(session, None, task_id)
        attempt = new_attempt(
            session,
            HUMAN,
            task,
            provider=provider,
            adapter="command",
            status=AttemptStatus.RUNNING,
            model=model,
        )
        finish_attempt(
            session, HUMAN, attempt, AttemptStatus(status), exit_reason="golden", duration_ms=duration_ms
        )
        attempt.queue_wait_ms = queue_ms


def _accept(app: Workgraph, ws: str, key: str) -> None:
    app.tasks.claim(HUMAN, ws, key, ClaimRequest())
    app.tasks.complete(HUMAN, ws, key)
    app.verification.accept_manually(HUMAN, ws, key, reason="golden")


def _configure(app: Workgraph, *, agents: bool) -> None:
    config = app.config
    config.llm.auto_classify = False
    config.llm.auto_embed = False
    config.llm.summarization.provider = "heuristic"
    config.llm.summarization.model = "heuristic-v1"
    config.execution.enabled = agents
    app.config_store.save_global(config)


def build_generated(app: Workgraph, seed: int, n_tasks: int, repo_root: Path) -> None:
    """Synthetic workspace: tasks, attempts, relations, git commits, acceptance and change surfaces."""
    rng = random.Random(seed)  # noqa: S311 - reproducible test data, not security
    _configure(app, agents=False)
    app.workspaces.create(
        HUMAN, WorkspaceCreate(slug="main", name="Main", key_prefix="WG"), make_default=True
    )
    repos = {}
    for name in REPO_NAMES[: 2 + seed % 3]:
        path = _init_repo(repo_root / name)
        _commit(path, "initial commit", {"README.md": f"# {name}\n"}, GIT_DATE)
        app.repositories.register(HUMAN, "main", RepositoryRegister(path=str(path), name=name))
        repos[name] = path
    app.projects.create(HUMAN, "main", _project("platform", "Platform", list(repos)))
    app.projects.create(
        HUMAN,
        "main",
        _project("frontend", "Frontend", [n for n in repos if "web" in n or "docs" in n] or list(repos)),
    )
    names = list(repos)
    keys: list[str] = []
    ids: dict[str, str] = {}
    shared: dict[str, tuple[str, str]] = {}
    for i in range(n_tasks):
        chosen = rng.sample(names, k=rng.choice([1, 1, 2]))
        priority = rng.choice(["low", "medium", "high", "critical"])
        task = app.tasks.create(
            HUMAN,
            "main",
            TaskCreate(
                title=f"Generated task {seed}-{i} about {rng.choice(['login', 'cache', 'billing', 'docs'])}",
                description="Generated for golden vectors.",
                repositories=chosen,
                projects=["platform" if len(chosen) > 1 else "frontend"],
                priority=priority,
                status="ready",
            ),
        )
        keys.append(task.key)
        ids[task.key] = task.id
        for _ in range(rng.randint(1, 3)):
            if rng.random() < 0.7:
                for repo in chosen:
                    body = {}
                    for f in range(rng.randint(1, 4)):
                        body[f"src/{repo}/mod{rng.randint(0, 5)}.py"] = "".join(
                            f"x{i}_{f}_{k} = {rng.randint(0, 99)}\n" for k in range(rng.randint(1, 12))
                        )
                    message = f"change {i} in {repo}\n\nTask-ID: {task.key}"
                    when = (EPOCH + TICK * (rng.randint(1, 200) + i)).strftime("%Y-%m-%dT%H:%M:%S+0000")
                    _commit(repos[repo], message, body, when)
        if i % 5 == 4:
            sha = _commit(repos[chosen[0]], "shared fix\n", {"src/shared.py": f"s = {i}\n"}, GIT_DATE)
            shared[task.key] = (chosen[0], sha)
    for name in names:
        app.git.sync_ref(HUMAN, "main", name)
    for key in keys:
        for _ in range(rng.randint(1, 3)):
            _add_attempt(
                app,
                ids[key],
                provider=rng.choice(["alpha", "beta", "gamma"]),
                model=rng.choice(["m1", "m2", None]),
                status=rng.choice(["succeeded", "succeeded", "failed", "timed_out", "cancelled"]),
                duration_ms=rng.randint(500, 900_000),
                queue_ms=rng.randint(0, 60_000),
            )
        if key in shared and rng.random() < 0.8:
            repo, sha = shared[key]
            app.git.link_commit(HUMAN, "main", key, sha, repository=repo)
        if rng.random() < 0.6:
            _accept(app, "main", key)
    for i, key in enumerate(keys):
        if i > 2 and rng.random() < 0.3:
            target = keys[rng.randrange(0, i)]
            relation = rng.choice(["regression_of", "follows_up", "relates_to"])
            app.tasks.add_relation(HUMAN, "main", key, RelationCreate(target=target, relation_type=relation))
    for key in keys:
        task_id = ids[key]
        with app.db.session() as session:
            accepted = session.get(Task, task_id).accepted_at is not None
        if accepted:
            app.git.compute_change_surface(HUMAN, task_id)


def _project(slug: str, name: str, repositories: list[str]) -> Any:
    from workgraph.core.models.schemas import ProjectCreate

    return ProjectCreate(slug=slug, name=name, repositories=repositories)


def build_demo(app: Workgraph) -> None:
    seed_demo(app, "demo", log=lambda _msg: None)


# ---------------------------------------------------------------- canonical labels
def id_labels(db_file: Path) -> dict[str, str]:
    con = sqlite3.connect(f"file:{db_file.as_posix()}?mode=ro", uri=True)
    try:
        labels: dict[str, str] = {}
        for tid, key in con.execute("SELECT id, key FROM tasks"):
            labels[tid] = f"task:{key}"
        for pid, slug in con.execute("SELECT id, slug FROM projects"):
            labels[pid] = f"project:{slug}"
        for rid, name in con.execute("SELECT id, name FROM repositories"):
            labels[rid] = f"repository:{name}"
        for cid, sha in con.execute("SELECT id, sha FROM commits"):
            labels[cid] = f"commit:{sha}"
        for aid, tid, number in con.execute("SELECT id, task_id, number FROM agent_attempts"):
            labels[aid] = f"attempt:{labels.get(tid, tid)}#{number}"
        tables = [
            r[0]
            for r in con.execute(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND sql NOT LIKE 'CREATE VIRTUAL%'"
            )
        ]
        for table in sorted(tables):
            cols = [r[1] for r in con.execute(f"PRAGMA table_info({table})")]
            if "id" not in cols:
                continue
            for rowid, value in con.execute(f"SELECT rowid, id FROM {table}"):
                if isinstance(value, str) and UUID_RE.fullmatch(value):
                    labels.setdefault(value, f"{table}:{rowid}")
        return labels
    finally:
        con.close()


UNLABELLED: list[str] = []


def canon_text(text: str, labels: dict[str, str]) -> str:
    def repl(match: re.Match[str]) -> str:
        found = match.group(0)
        if found not in labels:
            # Identifiers that no persisted row owns (for example a request correlation id stored in
            # event data) are random per run; they are replaced by a fixed placeholder.
            start = max(0, match.start() - 60)
            UNLABELLED.append(text[start : match.end() + 10])
            return "<unlabelled-uuid>"
        return labels[found]

    return UUID_RE.sub(repl, text)


def canon(value: Any, labels: dict[str, str]) -> Any:
    return json.loads(canon_text(json.dumps(value, sort_keys=True, ensure_ascii=False, default=str), labels))


SCRATCH: list[Path] = []
MACHINE_TEXT = ("Windows-10-10.0.19045-SP0",)


def scrub(text: str) -> str:
    """Replace machine-specific locations in the JSON text (raw and JSON-escaped spellings)."""
    for root, label in ((SCRATCH[0], "<scratch>"), (ROOT, "<repository>")):
        for form in {str(root), root.as_posix()}:
            for spelling in {form, json.dumps(form)[1:-1]}:
                text = text.replace(spelling, label)
    for fragment in MACHINE_TEXT:
        text = text.replace(fragment, "<platform>")
    # Paths recorded inside JSON strings are escaped twice, so match them by pattern too.
    text = re.sub(r"gv15-[A-Za-z0-9_]+", "gv15-<tmp>", text)
    return re.sub(
        r"[A-Za-z]:(?:\\\\|\\|/)+Users(?:\\\\|\\|/)+[^\\/\"]+(?:(?:\\\\|\\|/)+AppData(?:\\\\|\\|/)+Local(?:\\\\|\\|/)+Temp)?",
        "<home>",
        text,
    )


def write_json(path: Path, value: Any, labels: dict[str, str]) -> None:
    text = scrub(json.dumps(canon(value, labels), sort_keys=True, indent=1, ensure_ascii=False)) + "\n"
    data = text.encode("utf-8")
    if len(data) > MAX_FILE_BYTES:
        raise SystemExit(f"{path.name} is {len(data)} bytes, over the 2 MB limit")
    path.write_bytes(data)


def dump_source(db_file: Path, labels: dict[str, str]) -> dict[str, Any]:
    """The rows the warehouse loads: ``TABLES`` of ``analytics/warehouse.py``, the columns it selects.

    Tables the analytics never read (jobs, workers, ...) are left out; their timestamps depend on
    thread scheduling and would make two runs differ without changing any output.
    """
    from workgraph.analytics.warehouse import TABLES

    con = sqlite3.connect(f"file:{db_file.as_posix()}?mode=ro", uri=True)
    try:
        tables: dict[str, Any] = {}
        for name in sorted(TABLES):
            available = [r[1] for r in con.execute(f"PRAGMA table_info({name})")]
            wanted = TABLES[name]
            columns = [c for c in (wanted or available) if c in available]
            cursor = con.execute(f"SELECT {', '.join(columns)} FROM {name}")
            rows = []
            for row in cursor.fetchall():
                rows.append([base64.b64encode(v).decode("ascii") if isinstance(v, bytes) else v for v in row])
            tables[name] = {"columns": columns, "rows": rows}
        return tables
    finally:
        con.close()


def _plain(value: Any) -> Any:
    """Dates from DuckDB (period buckets) become ISO strings, as the service already does for datetimes."""
    if isinstance(value, datetime):
        return value.isoformat()
    if hasattr(value, "isoformat"):
        return value.isoformat()
    return value


def _sort_key(row: dict[str, Any], group_by: list[str]) -> tuple[str, ...]:
    return tuple(json.dumps(row.get(g), sort_keys=True) for g in group_by)


def sampled_queries() -> list[dict[str, Any]]:
    """The sampled case set, the same for every database.

    Not the cross product of metrics, dimensions and periods. Per metric:

    * the ungrouped query and the grouping by ``period`` under every period (five cases), so every
      metric sees every period and every ``date_trunc`` bucket rule;
    * one query per allowed dimension, the period rotating with the running index, so each dimension
      meets several periods and every (dimension, period) pair appears in some case;
    * one pair of dimensions (rotating through the allowed ones) and one ``period`` plus dimension query.

    Then a few filtered queries (time window, status, category, priority, project, repository, model,
    provider, kind) because filters add WHERE clauses the Rust port must reproduce.
    """
    periods = sorted(PERIODS)
    cases: list[dict[str, Any]] = []
    index = 0
    for metric in METRICS:
        dims = sorted(BASE_DIMENSIONS[metric.base])
        base = {"metric": metric.id}
        cases.append({**base, "group_by": [], "period": periods[index % 5], "filters": {}})
        for period in periods:
            cases.append({**base, "group_by": ["period"], "period": period, "filters": {}})
        for dim in dims:
            index += 1
            cases.append({**base, "group_by": [dim], "period": periods[index % 5], "filters": {}})
        if len(dims) >= 2:
            first = dims[index % len(dims)]
            second = dims[(index + 1) % len(dims)]
            index += 1
            cases.append({**base, "group_by": [first, second], "period": periods[index % 5], "filters": {}})
        index += 1
        cases.append(
            {
                **base,
                "group_by": ["period", dims[index % len(dims)]],
                "period": periods[index % 5],
                "filters": {},
            }
        )
    return cases


FILTER_CASES: list[dict[str, Any]] = [
    {
        "metric": "tasks_created",
        "group_by": ["period"],
        "period": "day",
        "filters": {"since": "SINCE", "until": "UNTIL"},
    },
    {
        "metric": "tasks_created",
        "group_by": ["category"],
        "period": "week",
        "filters": {"status": "accepted"},
    },
    {"metric": "tasks_accepted", "group_by": ["priority"], "period": "month", "filters": {"category": "bug"}},
    {"metric": "tasks_created", "group_by": [], "period": "week", "filters": {"priority": "high"}},
    {"metric": "tasks_created", "group_by": ["status"], "period": "week", "filters": {"project": "platform"}},
    {
        "metric": "tasks_created",
        "group_by": ["project"],
        "period": "week",
        "filters": {"repository": "FIRST_REPO"},
    },
    {"metric": "agent_duration", "group_by": ["provider"], "period": "week", "filters": {"model": "m1"}},
    {"metric": "agent_duration", "group_by": ["model"], "period": "week", "filters": {"provider": "alpha"}},
    {"metric": "check_failures", "group_by": ["kind"], "period": "week", "filters": {"kind": "test"}},
    {"metric": "execution_duration", "group_by": [], "period": "week", "filters": {"since": "SINCE"}},
]


def metric_cases(svc: Any, ws: str, repo: str) -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    since = EPOCH + timedelta(days=20)
    until = EPOCH + timedelta(days=90)
    queries = sampled_queries()
    for spec in FILTER_CASES:
        filters = {
            k: (since if v == "SINCE" else until if v == "UNTIL" else repo if v == "FIRST_REPO" else v)
            for k, v in spec["filters"].items()
        }
        queries.append({**spec, "filters": filters})
    for spec in queries:
        query = AnalyticsQuery(
            metric=spec["metric"],
            group_by=spec["group_by"],
            period=spec["period"],
            filters=AnalyticsFilters(**spec["filters"]),
        )
        try:
            result = svc.query(HUMAN, ws, query)
        except Exception as exc:  # a filter the metric does not support is itself a case
            cases.append({"query": query.model_dump(mode="json"), "error": type(exc).__name__})
            continue
        rows = [{k: _plain(v) for k, v in r.items()} for r in result["rows"]]
        rows.sort(key=lambda r: _sort_key(r, spec["group_by"]))
        cases.append(
            {
                "query": query.model_dump(mode="json"),
                "sample_size": result["sample_size"],
                "rows": rows,
            }
        )
    return cases


def report_outputs(app: Workgraph, ws: str) -> dict[str, Any]:
    svc = app.analytics
    windows_a = (EPOCH, EPOCH + timedelta(days=180))
    windows_b = (EPOCH + timedelta(days=180), EPOCH + timedelta(days=400))
    out: dict[str, Any] = {}
    out["overview"] = {p: svc.overview(HUMAN, ws, period=p) for p in sorted(PERIODS)}
    out["compare"] = {
        "tasks_created_by_category": svc.compare(
            HUMAN, ws, "tasks_created", a=windows_a, b=windows_b, group_by=["category"]
        ),
        "execution_duration": svc.compare(HUMAN, ws, "execution_duration", a=windows_a, b=windows_b),
        "cumulative_churn": svc.compare(HUMAN, ws, "cumulative_churn", a=windows_a, b=windows_b),
    }
    out["coupling"] = {
        "default": svc.coupling(HUMAN, ws),
        "min_support_2": svc.coupling(HUMAN, ws, min_support=2),
    }
    out["failures"] = svc.failures(HUMAN, ws)
    out["task_history"] = svc.task_history(HUMAN, ws, limit=50)
    with app.db.session():
        first_key = app.tasks.list(HUMAN, ws).items[0].key
    out["estimate"] = {
        "by_task": app.analytics.estimate(HUMAN, ws, task=first_key),
        "by_title": app.analytics.estimate(
            HUMAN, ws, title="Generated task about billing", description="invoice totals", category=None
        ),
    }
    out["retrospective"] = generate_retrospective(
        app, HUMAN, ws, since=EPOCH - timedelta(days=1), until=EPOCH + timedelta(days=400)
    )
    out["agent_time_bottlenecks"] = app.agent_time.bottlenecks(HUMAN, ws)
    out["subagents_week"] = app.subagent_report.report(HUMAN, ws, period="week")
    out["subagents_month"] = app.subagent_report.report(HUMAN, ws, period="month")
    return out


def run_database(name: str, home: Path, builder: Any, *, seed: int) -> None:
    t0 = time.perf_counter()
    home.mkdir(parents=True)
    with _clock_lock:
        _clock_ticks[0] = 0
    os.environ["WORKGRAPH_HOME"] = str(home)
    app = Workgraph(AppPaths.resolve(home))
    try:
        builder(app)
        print(f"{name}: built in {time.perf_counter() - t0:.1f}s", file=sys.stderr)
        db_file = app.paths.database_file
        labels = id_labels(db_file)
        ws = "demo" if name == "demo" else "main"
        write_json(
            OUT / f"input-{name}.json",
            {"database": name, "seed": seed, "tables": dump_source(db_file, labels)},
            labels,
        )
        svc = app.analytics
        t1 = time.perf_counter()
        write_json(
            OUT / f"metrics-{name}.json",
            {"database": name, "cases": metric_cases(svc, ws, "auth-service")},
            labels,
        )
        print(f"{name}: metrics {time.perf_counter() - t1:.1f}s", file=sys.stderr)
        write_json(
            OUT / f"reports-{name}.json", {"database": name, "reports": report_outputs(app, ws)}, labels
        )
    finally:
        app.close()
        os.environ.pop("WORKGRAPH_HOME", None)


def remove_tree(path: Path) -> None:
    """Delete a tree, clearing read-only flags (git object files are read-only on Windows)."""

    def clear_and_retry(func: Any, target: str, _exc: Any) -> None:
        Path(target).chmod(0o700)
        func(target)

    if path.exists():
        shutil.rmtree(path, onerror=clear_and_retry)


def main() -> None:
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    # Agent prompts and commits record the working paths, so the temporary home has a fixed name.
    scratch = (Path(tempfile.gettempdir()) / "wg-gv15-analytics").resolve()
    remove_tree(scratch)
    scratch.mkdir(parents=True)
    SCRATCH.append(scratch)
    # A hang in the demo's worker threads must not leave the script running.
    watchdog = threading.Timer(600, lambda: os._exit(3))
    watchdog.daemon = True
    watchdog.start()
    try:
        write_json(
            OUT / "definitions.json",
            {"metrics": [m.id for m in METRICS], "periods": sorted(PERIODS)},
            {},
        )
        run_database("demo", scratch / "demo-home", build_demo, seed=0)
        run_database(
            "gen",
            scratch / "gen-home",
            lambda app: build_generated(app, 1, 16, scratch / "gen-repos"),
            seed=1,
        )
    finally:
        watchdog.cancel()
        remove_tree(scratch)
    for path in sorted(OUT.iterdir()):
        print(f"{path.relative_to(ROOT).as_posix()} {path.stat().st_size}")
    print(f"unlabelled identifiers replaced: {len(UNLABELLED)}")
    for context in sorted(set(UNLABELLED))[:5]:
        print(f"  context: {context!r}")


if __name__ == "__main__":
    if os.environ.get("PYTHONHASHSEED") != "0":
        # Set iteration order over strings follows the hash seed; pin it so every run builds the same rows.
        child = subprocess.run(
            [sys.executable, str(Path(__file__).resolve())],
            env={**os.environ, "PYTHONHASHSEED": "0"},
            check=False,
        )
        raise SystemExit(child.returncode)
    main()
