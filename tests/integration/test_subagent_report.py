from __future__ import annotations

import json
from datetime import UTC, datetime
from typing import Any

from typer.testing import CliRunner

from workgraph.app import Workgraph
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.core.context import RequestContext
from workgraph.core.models.schemas import TaskCreate, WorkspaceCreate
from workgraph.core.services import resolve
from workgraph.experiments.stats import wilson
from workgraph.persistence.orm import (
    AgentSession,
    Commit,
    CommitTaskLink,
    Repository,
    SubagentRun,
    Task,
    TaskAcceptance,
    TaskChangeSurface,
)

HAIKU = "claude-haiku-5-5"
SONNET = "claude-sonnet-5-5"


def _at(day: str, hour: int = 12) -> datetime:
    return datetime.fromisoformat(f"{day}T{hour:02d}:00:00").replace(tzinfo=UTC)


def _seed(app: Workgraph, human: RequestContext, ws: str) -> dict[str, Any]:
    """Tasks with each outcome, commits and change surfaces, and subagent runs over two ISO weeks."""
    tasks = {
        key: app.tasks.create(human, ws, TaskCreate(title=key))
        for key in ("acc1", "acc2", "rej", "fail", "pend")
    }
    with app.db.session(write=True) as session:
        workspace_id = resolve.workspace(session, ws).id
        repo = Repository(workspace_id=workspace_id, name="repo")
        session.add(repo)
        session.flush()
        ids = {key: task.id for key, task in tasks.items()}
        for key, status in (
            ("acc1", "accepted"),
            ("acc2", "accepted"),
            ("rej", "awaiting_review"),
            ("fail", "failed"),
            ("pend", "running"),
        ):
            row = session.get(Task, ids[key])
            assert row is not None
            row.status = status

        def commit(sha: str, insertions: int, deletions: int, *, merge: bool = False) -> Commit:
            row = Commit(
                repository_id=repo.id,
                sha=sha,
                insertions=insertions,
                deletions=deletions,
                files_changed=1,
                is_merge=merge,
            )
            session.add(row)
            session.flush()
            return row

        def link(task_key: str, row: Commit) -> None:
            session.add(
                CommitTaskLink(commit_id=row.id, task_id=ids[task_key], source="trailer", confidence=1.0)
            )

        link("acc1", commit("c-acc1-a", 10, 3))
        link("acc1", commit("c-acc1-b", 5, 2))
        link("acc1", commit("c-acc1-merge", 500, 0, merge=True))
        link("acc2", commit("c-acc2", 1000, 0))
        link("rej", commit("c-rej", 4, 0))
        session.add(
            TaskChangeSurface(
                task_id=ids["acc2"],
                repository_id=repo.id,
                base_sha="a" * 40,
                final_sha="b" * 40,
                insertions=100,
                deletions=20,
                files_changed=3,
            )
        )
        session.add(TaskAcceptance(task_id=ids["acc1"], decision="accepted", mode="manual", actor="tester"))
        session.add(TaskAcceptance(task_id=ids["rej"], decision="rejected", mode="manual", actor="tester"))

        agent = AgentSession(provider="claude-code", external_session_id="report-session")
        session.add(agent)
        session.flush()

        def run(
            name: str,
            stopped: datetime | None,
            *,
            task: str | None = None,
            model: str | None = None,
            requested: str | None = None,
            output: int | None = None,
            turns: int | None = None,
            tool_errors: int = 0,
        ) -> None:
            usage = {"output_tokens": output} if output is not None else {}
            telemetry = {"tool_errors": tool_errors}
            if turns is not None:
                telemetry["turns"] = turns
            session.add(
                SubagentRun(
                    session_id=agent.id,
                    external_agent_id=name,
                    model=model,
                    requested_model=requested,
                    usage=usage,
                    telemetry=telemetry,
                    stopped_at=stopped,
                    task_id=ids[task] if task else None,
                )
            )

        run("r1", _at("2026-10-05"), task="acc1", model=HAIKU, output=1000, turns=10, tool_errors=1)
        run("r7", _at("2026-10-06"), task="acc1", output=None, turns=None)
        run("r2", _at("2026-10-07"), task="acc2", model=HAIKU, output=2000, turns=20)
        run("r2b", _at("2026-10-13", hour=0), task="acc2", model=HAIKU, output=4000, turns=40)
        run("r3", _at("2026-10-12"), task="rej", model=SONNET, output=3000, turns=30, tool_errors=2)
        run("r4", _at("2026-09-30"), task="fail", requested=HAIKU)
        run("r5", _at("2026-10-14"), task="pend", model=SONNET)
        run("r6", _at("2026-10-14"), model=HAIKU, output=500)
        run("r-open", None, task="pend", model=HAIKU, output=9999)
    return ids


def _by(rows: list[dict[str, Any]], **keys: Any) -> dict[str, Any]:
    matches = [row for row in rows if all(row[k] == v for k, v in keys.items())]
    assert len(matches) == 1, (keys, rows)
    return matches[0]


def test_buckets_outcomes_and_rates_by_week(app, human, ws):
    _seed(app, human, ws)
    report = app.subagent_report.report(human, ws, period="week")

    haiku_w41 = _by(report["by_period"], period="2026-W41", model=HAIKU)
    assert haiku_w41["runs"] == 2 and haiku_w41["linked_runs"] == 2
    assert haiku_w41["tasks"] == 1 and haiku_w41["accepted"] == 1
    # acc1 has exactly one haiku run, so it is a first pass; acc2 moved to W42 on its latest run.
    assert haiku_w41["first_pass"] == 1
    assert haiku_w41["median_output_tokens"] == 1500.0

    haiku_w42 = _by(report["by_period"], period="2026-W42", model=HAIKU)
    assert haiku_w42["runs"] == 2 and haiku_w42["linked_runs"] == 1
    assert haiku_w42["tasks"] == 1 and haiku_w42["accepted"] == 1
    assert haiku_w42["first_pass"] == 0

    haiku_w40 = _by(report["by_period"], period="2026-W40", model=HAIKU)
    assert haiku_w40["failed"] == 1 and haiku_w40["acceptance_rate"] == 0.0

    sonnet_w42 = _by(report["by_period"], period="2026-W42", model=SONNET)
    assert sonnet_w42["tasks"] == 2
    assert sonnet_w42["rejected"] == 1 and sonnet_w42["pending"] == 1
    assert sonnet_w42["acceptance_rate"] == 0.0
    assert sonnet_w42["median_turns"] == 30.0
    assert sonnet_w42["tool_errors"] == 2


def test_buckets_by_month(app, human, ws):
    _seed(app, human, ws)
    report = app.subagent_report.report(human, ws, period="month")

    september = _by(report["by_period"], period="2026-09", model=HAIKU)
    assert september["runs"] == 1 and september["failed"] == 1
    october = _by(report["by_period"], period="2026-10", model=HAIKU)
    assert october["runs"] == 4
    assert october["tasks"] == 2 and october["accepted"] == 2
    assert october["first_pass"] == 1


def test_totals_and_wilson_interval(app, human, ws):
    _seed(app, human, ws)
    report = app.subagent_report.report(human, ws)
    totals = _by(report["totals"], model=HAIKU)

    assert totals["runs"] == 5 and totals["linked_runs"] == 4
    assert totals["tasks"] == 3
    assert (totals["accepted"], totals["rejected"], totals["failed"], totals["pending"]) == (2, 0, 1, 0)
    assert totals["acceptance_rate"] == 2 / 3
    assert totals["wilson"] == list(wilson(2, 3))
    assert totals["first_pass"] == 1
    assert totals["median_output_tokens"] == 1500.0
    assert totals["tool_errors"] == 1

    sonnet = _by(report["totals"], model=SONNET)
    assert sonnet["wilson"] == list(wilson(0, 1))

    unknown = _by(report["totals"], model="unknown")
    assert unknown["runs"] == 1 and unknown["accepted"] == 1 and unknown["acceptance_rate"] == 1.0


def test_size_bands_prefer_change_surface_and_leave_out_merges(app, human, ws):
    _seed(app, human, ws)
    report = app.subagent_report.report(human, ws)

    haiku_rows = [row for row in report["by_size"] if row["model"] == HAIKU]
    assert [row["band"] for row in haiku_rows] == [4, 6, None]
    band4 = _by(haiku_rows, band=4)
    # Commits 10+3 and 5+2 lines; the merge commit is ignored.
    assert band4["tasks"] == 1 and band4["accepted"] == 1 and band4["median_lines"] == 20.0
    band6 = _by(haiku_rows, band=6)
    # The change surface (100+20) wins over the 1000-line commit.
    assert band6["accepted"] == 1 and band6["median_lines"] == 120.0
    unknown = _by(haiku_rows, band=None)
    assert unknown["tasks"] == 1 and unknown["accepted"] == 0 and unknown["decided"] == 1
    assert unknown["median_lines"] is None

    sonnet_rows = [row for row in report["by_size"] if row["model"] == SONNET]
    assert _by(sonnet_rows, band=2)["acceptance_rate"] == 0.0
    assert _by(sonnet_rows, band=None)["decided"] == 0


def test_model_filter_and_window(app, human, ws):
    _seed(app, human, ws)

    only_haiku = app.subagent_report.report(human, ws, model=HAIKU)
    assert {row["model"] for row in only_haiku["by_period"]} == {HAIKU}
    assert [row["model"] for row in only_haiku["totals"]] == [HAIKU]
    assert {row["model"] for row in only_haiku["by_size"]} == {HAIKU}

    # since is inclusive and until exclusive: r2 (10-07), r7 (10-06) and r3 (10-12) are in; r2b at 10-13T00:00 is out.
    windowed = app.subagent_report.report(
        human, ws, since=datetime(2026, 10, 6), until=datetime(2026, 10, 13)
    )
    haiku = _by(windowed["totals"], model=HAIKU)
    assert haiku["runs"] == 1 and haiku["tasks"] == 1 and haiku["accepted"] == 1
    assert _by(windowed["totals"], model=SONNET)["runs"] == 1
    assert windowed["window"]["since"].startswith("2026-10-06")
    assert windowed["unlinked_runs"] == 0


def test_unlinked_runs_and_notes(app, human, ws):
    _seed(app, human, ws)
    report = app.subagent_report.report(human, ws)

    assert report["unlinked_runs"] == 1
    assert any(
        note == "1 runs have no Task-ID link; start a subagent prompt with `Task-ID: <KEY>` to link it"
        for note in report["notes"]
    )
    assert any("no stopped_at" in note for note in report["notes"])

    other = app.workspaces.create(
        human, WorkspaceCreate(slug="other", name="Other", key_prefix="OT"), make_default=False
    )
    elsewhere = app.subagent_report.report(human, other.slug)
    assert elsewhere["unlinked_runs"] == 1
    assert elsewhere["totals"] == []
    assert any("other workspaces" in note for note in elsewhere["notes"])
    assert any("default workspace" in note for note in elsewhere["notes"])


def test_report_is_json_serialisable(app, human, ws):
    _seed(app, human, ws)
    report = app.subagent_report.report(human, ws, period="month", since=datetime(2026, 9, 1))
    assert json.loads(json.dumps(report)) == report


def test_cli_json_and_human_output(app, human, ws):
    _seed(app, human, ws)
    runner = CliRunner()

    def invoke(*args: str, json_mode: bool) -> str:
        STATE.json = json_mode
        STATE.home = None
        try:
            result = runner.invoke(cli, list(args), catch_exceptions=False)
        finally:
            STATE.close()
            STATE.json = False
        assert result.exit_code == 0, result.output
        return result.output

    payload = json.loads(
        invoke(
            "analytics",
            "subagents",
            "--period",
            "week",
            "--since",
            "2026-10-06",
            "--until",
            "2026-10-13",
            "--model",
            HAIKU,
            json_mode=True,
        )
    )
    assert payload["period"] == "week"
    assert {row["model"] for row in payload["by_period"]} == {HAIKU}
    assert _by(payload["totals"], model=HAIKU)["accepted"] == 1

    invoke("analytics", "subagents", json_mode=False)
