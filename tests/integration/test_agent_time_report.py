from __future__ import annotations

import json
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from typer.testing import CliRunner

from workgraph.app import Workgraph
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.core.context import RequestContext
from workgraph.core.models.schemas import TaskCreate
from workgraph.core.services import resolve
from workgraph.persistence.orm import AgentSession, SubagentRun, Task

HAIKU = "claude-haiku-5-5"
SONNET = "claude-sonnet-5-5"


def _at(hour: int, minute: int = 0) -> datetime:
    return datetime(2026, 10, 14, hour, minute, tzinfo=UTC)


def _iso(hour: int, minute: int = 0, second: int = 0) -> str:
    return datetime(2026, 10, 14, hour, minute, second, tzinfo=UTC).isoformat().replace("+00:00", "Z")


def _time(
    *,
    span: float,
    model: float = 0.0,
    waiting: float = 0.0,
    classes: dict[str, tuple[float, int]] | None = None,
    slowest: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    classes = classes or {}
    tool = sum(seconds for seconds, _ in classes.values())
    return {
        "version": 1,
        "span_seconds": span,
        "tool_seconds": tool,
        "model_seconds": model,
        "waiting_seconds": waiting,
        "classes": {name: {"seconds": seconds, "calls": calls} for name, (seconds, calls) in classes.items()},
        "slowest": slowest or [],
    }


def _call(cls: str, seconds: float, command: str, at: str) -> dict[str, Any]:
    return {"class": cls, "seconds": seconds, "tool": "Bash", "command": command, "at": at}


def _seed(app: Workgraph, human: RequestContext, ws: str) -> dict[str, str]:
    """Two linked runs that overlap, an unlinked run, a trial, a run without a breakdown, and a run before the window."""
    task = app.tasks.create(human, ws, TaskCreate(title="timed work"))
    with app.db.session(write=True) as session:
        workspace_id = resolve.workspace(session, ws).id
        row = session.get(Task, task.id)
        assert row is not None and row.workspace_id == workspace_id
        agent = AgentSession(provider="claude-code", external_session_id="time-session")
        session.add(agent)
        session.flush()

        def run(
            name: str,
            start: datetime,
            stop: datetime,
            *,
            model: str,
            time: dict[str, Any] | None = None,
            linked: bool = False,
            description: str | None = None,
            trial: bool = False,
        ) -> None:
            telemetry: dict[str, Any] = {}
            if time is not None:
                telemetry["time"] = time
            if description is not None:
                telemetry["description"] = description
            if trial:
                telemetry["trial_ids"] = ["trial-1"]
            session.add(
                SubagentRun(
                    session_id=agent.id,
                    external_agent_id=name,
                    model=model,
                    started_at=start,
                    stopped_at=stop,
                    telemetry=telemetry,
                    task_id=task.id if linked else None,
                )
            )

        run(
            "implementer",
            _at(10),
            _at(10, 10),
            model=HAIKU,
            linked=True,
            description="implementer",
            time=_time(
                span=600,
                model=100,
                classes={"cargo test": (240, 2), "search/read": (60, 5)},
                slowest=[_call("cargo test", 240, "cargo test -p x", _iso(10, 1))],
            ),
        )
        run(
            "poller",
            _at(10, 2),
            _at(10, 8),
            model=HAIKU,
            linked=True,
            description="reviewer",
            time=_time(
                span=360,
                waiting=300,
                classes={"polling loop": (200, 1)},
                slowest=[_call("polling loop", 200, "until test -f x; do sleep 5; done", _iso(10, 3))],
            ),
        )
        run(
            "unlinked",
            _at(10),
            _at(10, 1),
            model=SONNET,
            time=_time(span=60, model=50, classes={"search/read": (10, 1)}),
        )
        run(
            "trial",
            _at(10),
            _at(10, 0),
            model=HAIKU,
            trial=True,
            time=_time(span=30, classes={"git": (30, 1)}),
        )
        run("untimed", _at(9), _at(9, 1), model=HAIKU, linked=True, time=None)
    return {"task": task.key}


def test_breakdown_by_class_sums_tool_model_and_waiting(app, human, ws):
    _seed(app, human, ws)
    report = app.agent_time.breakdown(human, ws)

    rows = {row["class"]: row for row in report["rows"]}
    assert rows["cargo test"]["seconds"] == 240 and rows["cargo test"]["calls"] == 2
    assert rows["polling loop"]["seconds"] == 200
    assert (
        rows["waiting on background"]["seconds"] == 300 and rows["waiting on background"]["kind"] == "waiting"
    )
    assert rows["model"]["seconds"] == 150 and rows["model"]["kind"] == "model"
    assert "git" not in rows  # the trial is excluded
    assert report["totals"]["runs"] == 3
    assert report["totals"]["accounted_seconds"] == 240 + 60 + 200 + 10 + 150 + 300
    assert abs(sum(row["share"] for row in report["rows"]) - 1.0) < 1e-9
    assert any("no time breakdown" in note for note in report["notes"])


def test_breakdown_by_model_task_and_description(app, human, ws):
    seeded = _seed(app, human, ws)
    by_model = app.agent_time.breakdown(human, ws, by="model")
    assert {row["key"] for row in by_model["rows"]} == {HAIKU, SONNET}

    by_task = app.agent_time.breakdown(human, ws, by="task")
    keys = {row["key"]: row for row in by_task["rows"]}
    assert keys[seeded["task"]]["runs"] == 2
    assert keys["(unlinked)"]["runs"] == 1

    by_description = app.agent_time.breakdown(human, ws, by="description")
    assert {row["key"] for row in by_description["rows"]} == {"implementer", "reviewer", "unknown"}


def test_window_and_model_filters(app, human, ws):
    _seed(app, human, ws)
    late = app.agent_time.breakdown(human, ws, since=_at(10, 9))
    assert late["totals"]["runs"] == 1  # only the implementer stopped after 10:09

    haiku = app.agent_time.breakdown(human, ws, model=HAIKU)
    assert haiku["totals"]["runs"] == 2


def test_bottlenecks_report_concurrency_and_wall_time(app, human, ws):
    seeded = _seed(app, human, ws)
    report = app.agent_time.bottlenecks(human, ws, top=5)

    slowest = report["slowest_calls"]
    assert [call["class"] for call in slowest] == ["cargo test", "polling loop"]
    # At 10:01 the implementer (10:00-10:10) and the unlinked run (10:00-10:01) were live.
    assert slowest[0]["concurrent_runs"] == 2
    # At 10:03 the implementer and the poller (10:02-10:08) were live.
    assert slowest[1]["concurrent_runs"] == 2
    # At 10:00 the implementer, the unlinked run and the trial all started; concurrency is machine-wide.
    assert report["concurrency"]["peak_during_window"] == 3

    by_task = {row["key"]: row for row in report["by_task"]}
    assert by_task[seeded["task"]]["span_seconds"] == 960
    assert by_task[seeded["task"]]["top_classes"][0] == {"class": "cargo test", "seconds": 240.0}
    assert by_task[seeded["task"]]["polling_share"] is not None
    assert report["totals"]["waiting_share"] is not None


def test_reports_are_json_serialisable(app, human, ws):
    _seed(app, human, ws)
    json.dumps(app.agent_time.breakdown(human, ws, by="task"))
    json.dumps(app.agent_time.bottlenecks(human, ws))


def test_cli_time_and_bottlenecks(app, human, ws):
    _seed(app, human, ws)
    runner = CliRunner()
    for args in (["analytics", "time"], ["analytics", "time", "--by", "task"], ["analytics", "bottlenecks"]):
        STATE.json = True
        STATE.home = None
        try:
            result = runner.invoke(cli, list(args), catch_exceptions=False)
        finally:
            STATE.close()
            STATE.json = False
        assert result.exit_code == 0, result.output
        assert json.loads(result.output)


def test_backfill_time_stores_breakdown_once_and_leaves_run_state_alone(app, human, ws, tmp_path: Path):
    records = [
        {"type": "user", "timestamp": _iso(11), "message": {"role": "user", "content": "go"}},
        {
            "type": "assistant",
            "timestamp": _iso(11, 0, 4),
            "message": {
                "id": "m1",
                "model": HAIKU,
                "content": [
                    {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "cargo test"}}
                ],
            },
        },
        {
            "type": "user",
            "timestamp": _iso(11, 0, 34),
            "message": {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "t1", "content": ""}],
            },
        },
    ]
    transcript = tmp_path / "agent-old.jsonl"
    transcript.write_text("\n".join(json.dumps(record) for record in records) + "\n", encoding="utf-8")
    with app.db.session(write=True) as session:
        agent = AgentSession(provider="claude-code", external_session_id="backfill-session")
        session.add(agent)
        session.flush()
        session.add(
            SubagentRun(
                session_id=agent.id,
                external_agent_id="old",
                started_at=_at(11),
                stopped_at=_at(11, 1),
                transcript_path=str(transcript),
                telemetry={"tool_errors": 0},
            )
        )

    first = app.hooks.backfill_time(human)
    second = app.hooks.backfill_time(human)

    assert first == {"parsed": 1, "missing": 0, "skipped": 0}
    assert second == {"parsed": 0, "missing": 0, "skipped": 1}
    with app.db.session() as session:
        run = session.query(SubagentRun).filter_by(external_agent_id="old").one()
        assert run.telemetry["time"]["classes"]["cargo test"]["seconds"] == 30
        assert run.telemetry["tool_errors"] == 0
        assert run.model is None  # the backfill writes only the breakdown
