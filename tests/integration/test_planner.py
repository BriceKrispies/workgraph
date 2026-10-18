from __future__ import annotations

import json
from typing import Any

import pytest
from typer.testing import CliRunner

from workgraph.app import Workgraph
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.config.paths import AppPaths
from workgraph.core.context import RequestContext
from workgraph.core.models.schemas import TaskCreate, TrialOut, WorkspaceCreate
from workgraph.experiments.stats import wilson
from workgraph.persistence.orm import AgentSession, SubagentRun

HAIKU = "claude-haiku-5-5"
SONNET = "claude-sonnet-5-5"
POOL = "experiment-pool"
PASS_MAX_BAND = 4


def _protocol(**overrides: Any) -> dict[str, Any]:
    base: dict[str, Any] = {
        "arms": [{"name": "haiku", "model": HAIKU}, {"name": "control", "model": SONNET}],
        "brief_template": "leaf-v1",
        "primary_size": "source_lines",
        "bands": {"min": 2, "max": 8},
        "staircase": {"per_visit": 2, "up": 1, "down": 2, "max_reversals": 2, "max_trials": 12},
        "refine": {"trials": 2, "width": 1},
        "repeat": {"trials": 1},
        "control": {"arm": "control", "trials": 2},
        "budget": {"trials": 40},
        "seed": 7,
    }
    base.update(overrides)
    return base


def _replay(band: int, *, validated: bool = True) -> dict[str, Any]:
    features = {"src_lines": 2**band + 1, "test_lines": 4, "src_files": 1, "packages": ["core"], "band": band}
    return {"validated": validated, "features": features}


def _make_pool(
    app: Workgraph,
    human: RequestContext,
    ws: str,
    per_band: dict[int, int] | None = None,
    *,
    unvalidated: bool = False,
) -> None:
    counts = per_band if per_band is not None else dict.fromkeys(range(2, 9), 3)
    for band in sorted(counts):
        for index in range(counts[band]):
            app.tasks.create(
                human,
                ws,
                TaskCreate(title=f"band {band} #{index}", tags=[POOL], metadata={"replay": _replay(band)}),
            )
    if unvalidated:
        app.tasks.create(
            human,
            ws,
            TaskCreate(title="unvalidated", tags=[POOL], metadata={"replay": _replay(8, validated=False)}),
        )


def _start(app: Workgraph, human: RequestContext, ws: str, protocol: dict[str, Any] | None = None) -> str:
    app.experiments.create(human, ws, "planner", "Planner", None, protocol or _protocol())
    app.experiments.freeze(human, ws, "planner")
    return "planner"


def _passes(band: int | None) -> bool:
    return band is not None and band <= PASS_MAX_BAND


def _grade(app: Workgraph, human: RequestContext, ws: str, trial: TrialOut) -> TrialOut:
    app.experiments.start_trial(human, ws, trial.id)
    return app.experiments.record_grade(human, ws, trial.id, "pass" if _passes(trial.band) else "fail")


def _drive(
    app: Workgraph, human: RequestContext, ws: str, slug: str
) -> tuple[list[TrialOut], dict[str, Any]]:
    """Plan and grade trials until the planner says done. Returns the planned trials and the final answer."""
    planned: list[TrialOut] = []
    for _ in range(100):
        out = app.trial_planner.next(human, ws, slug)
        if out["done"]:
            return planned, out
        assert out["reason"] == "planned", out["reason"]
        planned.append(out["trial"])
        _grade(app, human, ws, out["trial"])
    raise AssertionError("the planner did not finish")


def _add_graded(
    app: Workgraph,
    human: RequestContext,
    ws: str,
    slug: str,
    band: int,
    passed: bool,
    *,
    tag: str = "",
) -> TrialOut:
    """A measured trial added directly, for report fixtures. Phase ``live`` keeps it out of the staircase."""
    task = app.tasks.create(
        human, ws, TaskCreate(title=f"measured {band} {tag}", tags=[POOL], metadata={"replay": _replay(band)})
    )
    trial = app.experiments.add_trial(
        human, ws, slug, task.key, "haiku", "live", band=band, features={"replay": _replay(band)["features"]}
    )
    app.experiments.start_trial(human, ws, trial.id)
    return app.experiments.record_grade(human, ws, trial.id, "pass" if passed else "fail")


# Passes out of six tasks per band: a smooth success curve that crosses 50% near band 4.5.
PASS_COUNTS = {2: 6, 3: 5, 4: 4, 5: 2, 6: 1, 7: 0, 8: 0}


def test_first_pick_is_top_band_and_a_pending_trial_is_returned(app, human, ws):
    _make_pool(app, human, ws, unvalidated=True)
    slug = _start(app, human, ws)
    first = app.trial_planner.next(human, ws, slug)
    assert (first["done"], first["phase"], first["reason"]) == (False, "staircase", "planned")
    trial = first["trial"]
    assert trial.band == 8 and first["step"]["band"] == 8
    assert app.tasks.get(human, ws, trial.task_key).title != "unvalidated"

    again = app.trial_planner.next(human, ws, slug)
    assert (again["reason"], again["trial"].id) == ("pending", trial.id)
    assert len(app.experiments.trials(human, ws, slug)) == 1


def test_sequence_runs_staircase_refine_repeat_control_then_completes(app, human, ws):
    _make_pool(app, human, ws, unvalidated=True)
    slug = _start(app, human, ws)
    planned, done = _drive(app, human, ws, slug)
    assert (done["done"], done["reason"], done["trial"]) == (True, "complete", None)

    def phase(name: str) -> list[TrialOut]:
        return [t for t in planned if t.phase == name]

    assert [t.band for t in phase("staircase")] == [8, 8, 6, 6, 4, 4, 5, 5]
    assert [t.band for t in phase("refine")] == [4, 5]
    assert len(phase("repeat")) == 1
    control = phase("control")
    assert [t.arm for t in control] == ["control", "control"]
    assert all(t.band in (4, 5) for t in control)

    primary = [t for t in planned if t.arm == "haiku" and t.phase != "repeat"]
    primary_keys = [t.task_key for t in primary]
    assert len(primary_keys) == len(set(primary_keys)) == 10
    assert phase("repeat")[0].task_key in set(primary_keys)
    assert len({t.task_key for t in control}) == 2
    titles = {app.tasks.get(human, ws, t.task_key).title for t in planned}
    assert "unvalidated" not in titles


def test_same_seed_and_history_pick_the_same_tasks(app, human, ws):
    app.workspaces.create(
        human, WorkspaceCreate(slug="twin", name="Twin", key_prefix="TW"), make_default=False
    )
    picks: list[list[tuple[str, int | None, str]]] = []
    for workspace in (ws, "twin"):
        _make_pool(app, human, workspace)
        slug = _start(app, human, workspace)
        planned, _ = _drive(app, human, workspace, slug)
        picks.append([(t.phase, t.band, t.task_key.rsplit("-", 1)[1]) for t in planned])
    assert picks[0] == picks[1]
    assert len(picks[0]) == 13


def test_invalid_trial_does_not_count_and_its_band_is_run_again(app, human, ws):
    _make_pool(app, human, ws)
    slug = _start(app, human, ws, _protocol(budget={"trials": 1}))
    first = app.trial_planner.next(human, ws, slug)["trial"]
    _grade(app, human, ws, first)
    assert app.trial_planner.next(human, ws, slug)["reason"] == "budget"

    app.experiments.mark_invalid(human, ws, first.id, "harness", "runner crashed")
    rerun = app.trial_planner.next(human, ws, slug)
    assert (rerun["done"], rerun["reason"]) == (False, "planned")
    assert rerun["trial"].band == 8 and rerun["trial"].seq == 2
    assert rerun["step"]["visits"] == []


def test_band_is_substituted_when_its_tasks_are_used_up(app, human, ws):
    _make_pool(app, human, ws, {**dict.fromkeys(range(2, 8), 3), 8: 1})
    slug = _start(app, human, ws)
    first = app.trial_planner.next(human, ws, slug)["trial"]
    assert first.band == 8
    _grade(app, human, ws, first)
    second = app.trial_planner.next(human, ws, slug)
    assert second["reason"] == "planned"
    assert second["trial"].band == 7
    assert second["trial"].features["predicted"]["band_requested"] == 8
    assert second["step"]["band"] == 8


def test_pool_exhausted_when_no_unused_task_remains(app, human, ws):
    _make_pool(app, human, ws, {8: 1})
    slug = _start(app, human, ws)
    _grade(app, human, ws, app.trial_planner.next(human, ws, slug)["trial"])
    out = app.trial_planner.next(human, ws, slug)
    assert (out["done"], out["reason"], out["trial"]) == (True, "pool_exhausted", None)


def test_budget_stops_the_planner(app, human, ws):
    _make_pool(app, human, ws)
    slug = _start(app, human, ws, _protocol(budget={"trials": 3}))
    planned, done = _drive(app, human, ws, slug)
    assert len(planned) == 3
    assert (done["done"], done["reason"]) == (True, "budget")


def test_report_bands_fit_and_intervals(app, human, ws):
    slug = _start(app, human, ws, _protocol(budget={"trials": 100}))
    for band, passes in PASS_COUNTS.items():
        for index in range(6):
            _add_graded(app, human, ws, slug, band, index < passes, tag=str(index))

    report = app.trial_planner.report(human, ws, slug)
    json.dumps(report)
    assert report["experiment"]["slug"] == "planner" and report["step"]["phase"] == "staircase"
    haiku, control = report["arms"]
    assert [arm["arm"] for arm in report["arms"]] == ["haiku", "control"]
    assert haiku["bands"][0] == {"band": 2, "n": 6, "passes": 6, "rate": 1.0, "wilson": list(wilson(6, 6))}
    assert [row["band"] for row in haiku["bands"]] == [2, 3, 4, 5, 6, 7, 8]

    fit = haiku["fit"]
    assert fit["converged"] and fit["slope"] < 0 and fit["n"] == 42
    assert 2**4 < fit["lines50"] < 2**6
    assert fit["lines80"] < fit["lines50"]
    assert fit["ci50"] is not None and fit["ci50"][0] <= fit["ci50"][1]
    assert fit["ci80"] is not None and fit["ci80"][0] <= fit["ci80"][1]
    assert control["fit"] is None and "at least 4" in control["fit_reason"]


def test_fit_is_null_with_a_reason_below_four_trials(app, human, ws):
    slug = _start(app, human, ws, _protocol(budget={"trials": 100}))
    for index, band in enumerate([4, 5, 6]):
        _add_graded(app, human, ws, slug, band, index == 0, tag=str(index))
    arm = app.trial_planner.report(human, ws, slug)["arms"][0]
    assert arm["fit"] is None
    assert arm["fit_reason"] == "needs at least 4 graded trials with src_lines, has 3"


def test_report_lists_invalid_trials_and_failure_causes(app, human, ws):
    slug = _start(app, human, ws, _protocol(budget={"trials": 100}))
    failed = _add_graded(app, human, ws, slug, 6, False, tag="failed")
    app.experiments.classify(human, ws, failed.id, "too_big", "human")
    broken = _add_graded(app, human, ws, slug, 5, True, tag="broken")
    app.experiments.mark_invalid(human, ws, broken.id, "harness", "runner crashed")

    arm = app.trial_planner.report(human, ws, slug)["arms"][0]
    assert arm["failure_causes"] == {"harness": 1, "too_big": 1}
    assert arm["invalid"] == [
        {
            "seq": broken.seq,
            "task_key": broken.task_key,
            "failure_cause": "harness",
            "notes": "runner crashed",
        }
    ]
    assert [row["band"] for row in arm["bands"]] == [6]


def test_report_usage_comes_from_linked_subagent_runs(app, human, ws):
    slug = _start(app, human, ws, _protocol(budget={"trials": 100}))
    passed = _add_graded(app, human, ws, slug, 3, True, tag="passed")
    failed = _add_graded(app, human, ws, slug, 5, False, tag="failed")
    _add_graded(app, human, ws, slug, 3, True, tag="no-run")
    with app.db.session(write=True) as session:
        agent = AgentSession(provider="test", external_session_id="usage-session")
        session.add(agent)
        session.flush()
        pass_run = SubagentRun(
            session_id=agent.id,
            external_agent_id="pass-run",
            usage={"output_tokens": 1000, "cache_read_input_tokens": 400},
            telemetry={"turns": 10, "tool_errors": 0},
        )
        fail_run = SubagentRun(
            session_id=agent.id,
            external_agent_id="fail-run",
            usage={"output_tokens": 3000, "cache_read_input_tokens": 800},
            telemetry={"turns": 30, "tool_errors": 4},
        )
        session.add_all([pass_run, fail_run])
        session.flush()
        pass_run_id, fail_run_id = pass_run.id, fail_run.id
    app.experiments.link_run(human, ws, passed.id, subagent_run_id=pass_run_id)
    app.experiments.link_run(human, ws, failed.id, subagent_run_id=fail_run_id)

    usage = app.trial_planner.report(human, ws, slug)["arms"][0]["usage"]
    assert (usage["pass"]["n"], usage["pass"]["missing_usage"]) == (1, 1)
    assert usage["pass"]["output_tokens"] == {"median": 1000.0, "mean": 1000.0}
    assert usage["pass"]["turns"] == {"median": 10.0, "mean": 10.0}
    assert (usage["fail"]["n"], usage["fail"]["missing_usage"]) == (1, 0)
    assert usage["fail"]["output_tokens"] == {"median": 3000.0, "mean": 3000.0}
    assert usage["fail"]["cache_read_input_tokens"] == {"median": 800.0, "mean": 800.0}
    assert usage["fail"]["tool_errors"] == {"median": 4.0, "mean": 4.0}


@pytest.fixture
def run(home, monkeypatch, tmp_path):
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "claude-config"))
    runner = CliRunner()

    def invoke(*args: str, expect: int = 0, json_mode: bool = True) -> object:
        STATE.json = json_mode
        STATE.home = None
        try:
            result = runner.invoke(cli, list(args), catch_exceptions=False)
        finally:
            STATE.close()
            STATE.json = False
        assert result.exit_code == expect, f"{args}: exit {result.exit_code}\n{result.output}"
        text = result.output.strip()
        if json_mode and text.startswith(("{", "[")):
            return json.loads(text)
        return text

    return invoke


def test_cli_experiment_next_and_report(run, home):
    run("init")
    run("config", "set", "llm.auto_classify", "false")
    run("config", "set", "llm.auto_embed", "false")
    inst = Workgraph(AppPaths.resolve(home))
    try:
        ctx = RequestContext.human("tester")
        for band in range(2, 9):
            for index in range(2):
                inst.tasks.create(
                    ctx,
                    None,
                    TaskCreate(
                        title=f"band {band} #{index}", tags=[POOL], metadata={"replay": _replay(band)}
                    ),
                )
        inst.experiments.create(ctx, None, "cli-plan", "CLI plan", None, _protocol())
        inst.experiments.freeze(ctx, None, "cli-plan")
    finally:
        inst.close()

    first = run("experiment", "next", "cli-plan")
    assert (first["done"], first["reason"], first["trial"]["band"]) == (False, "planned", 8)
    again = run("experiment", "next", "cli-plan")
    assert (again["reason"], again["trial"]["id"]) == ("pending", first["trial"]["id"])

    report = run("experiment", "report", "cli-plan")
    assert report["experiment"]["slug"] == "cli-plan"
    assert [arm["arm"] for arm in report["arms"]] == ["haiku", "control"]
    text = run("experiment", "report", "cli-plan", json_mode=False)
    assert "haiku" in text and "fit: needs at least 4" in text
    assert run("experiment", "next", "cli-plan", json_mode=False).startswith("pending: ")
