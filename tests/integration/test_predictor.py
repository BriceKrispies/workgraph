from __future__ import annotations

import json
import math
import random
from collections.abc import Callable
from typing import Any

import pytest
from typer.testing import CliRunner

from workgraph.app import Workgraph
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.config.paths import AppPaths
from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError, ValidationFailed
from workgraph.core.models.schemas import PredictorFitOut, RepositoryRegister, TaskCreate, TrialOut
from workgraph.experiments.predictor import features_of

HAIKU = "claude-haiku-5-5"
SONNET = "claude-sonnet-5-5"


def _protocol() -> dict[str, Any]:
    return {
        "arms": [{"name": "haiku", "model": HAIKU}, {"name": "control", "model": SONNET}],
        "brief_template": "leaf-v1",
        "primary_size": "source_lines",
        "bands": {"min": 2, "max": 8},
        "staircase": {"per_visit": 2, "up": 1, "down": 2, "max_reversals": 2, "max_trials": 12},
        "refine": {"trials": 2, "width": 1},
        "repeat": {"trials": 1},
        "control": {"arm": "control", "trials": 2},
        "budget": {"trials": 200},
        "seed": 7,
    }


def _sigmoid(eta: float) -> float:
    return 1.0 / (1.0 + math.exp(-eta))


def _measured(
    app: Workgraph,
    human: RequestContext,
    ws: str | None,
    slug: str,
    *,
    lines: int,
    passed: bool,
    repo: str | None = "alpha",
    arm: str = "haiku",
    packages: tuple[str, ...] = ("core",),
    replay: bool = True,
) -> TrialOut:
    """One task, linked to ``repo``, run as one trial and graded. ``replay=False`` leaves out the features."""
    task = app.tasks.create(human, ws, TaskCreate(title=f"measured {lines} {arm} {passed}"))
    if repo is not None:
        app.tasks.link_repository(human, ws, task.key, repo)
    features = None
    if replay:
        band = round(math.log2(lines))
        features = {
            "replay": {"src_lines": lines, "src_files": 2, "packages": list(packages), "band": band},
        }
    trial = app.experiments.add_trial(
        human, ws, slug, task.key, arm, "live", band=round(math.log2(lines)), features=features
    )
    app.experiments.start_trial(human, ws, trial.id)
    return app.experiments.record_grade(human, ws, trial.id, "pass" if passed else "fail")


def _seed(app: Workgraph, human: RequestContext, ws: str | None, make_repo: Callable[..., Any]) -> str:
    """Repositories alpha and beta; 60 haiku trials on a known logistic rule (mostly alpha); 3 passing beta trials."""
    for name in ("alpha", "beta"):
        app.repositories.register(human, ws, RepositoryRegister(path=str(make_repo(name).path)))
    app.experiments.create(human, ws, "fit-data", "Fit data", None, _protocol())
    app.experiments.freeze(human, ws, "fit-data")
    rng = random.Random(11)
    for index in range(60):
        lines = 2 ** rng.randint(2, 8) + 1
        passed = rng.random() < _sigmoid(6 - math.log2(lines))
        packages = ("core",) if index % 3 else ("core", "cli")
        _measured(app, human, ws, "fit-data", lines=lines, passed=passed, repo="alpha", packages=packages)
    for lines in (9, 17, 33):
        _measured(app, human, ws, "fit-data", lines=lines, passed=True, repo="beta")
    return "fit-data"


@pytest.fixture
def seeded(app, human, ws, make_repo) -> str:
    return _seed(app, human, ws, make_repo)


def _repo_id(app: Workgraph, human: RequestContext, ws: str, name: str) -> str:
    return app.repositories.get(human, ws, name).id


def _coef_for_repo(fit: PredictorFitOut, repo_id: str) -> float:
    return fit.coef[4 + fit.repositories.index(repo_id)]


def test_fit_stores_row_with_cutoff_trials_and_samples(app, human, ws, seeded):
    fit = app.predictor.fit(human, ws, HAIKU, samples=200)
    trials = app.experiments.trials(human, ws, seeded)
    graded = [
        t for t in trials if t.status == "graded" and t.arm == "haiku" and "replay" in t.features["predicted"]
    ]
    assert fit.model == HAIKU
    assert fit.n == len(graded) == len(fit.trial_ids)
    assert set(fit.trial_ids) == {t.id for t in graded}
    assert fit.data_cutoff == max(t.graded_at for t in graded)
    assert fit.feature_names[:3] == ["log2_src_lines", "log2_src_files", "packages"]
    assert fit.repositories == sorted({_repo_id(app, human, ws, "alpha"), _repo_id(app, human, ws, "beta")})
    assert len(fit.coef) == len(fit.penalty) == 4 + len(fit.repositories)
    assert fit.penalty[0] == 0.0
    assert fit.coef[1] < 0, "more lines must lower the predicted success"
    assert fit.converged
    assert len(fit.samples) > 150
    assert all(len(sample) == len(fit.coef) for sample in fit.samples)
    assert fit.metrics["n_pass"] + fit.metrics["n_fail"] == fit.n
    assert 0.0 <= fit.metrics["brier"] <= 1.0
    assert app.predictor.latest(human, ws, HAIKU).id == fit.id


def test_fit_excludes_invalid_trials_other_models_and_featureless_trials(app, human, ws, seeded):
    _measured(app, human, ws, seeded, lines=17, passed=True, arm="control")
    _measured(app, human, ws, seeded, lines=17, passed=False, arm="control")
    _measured(app, human, ws, seeded, lines=17, passed=True, replay=False)
    invalid = app.experiments.add_trial(
        human,
        ws,
        seeded,
        app.tasks.create(human, ws, TaskCreate(title="wrong model")).key,
        "haiku",
        "live",
        band=4,
        features={"replay": {"src_lines": 17, "src_files": 2, "packages": ["core"]}},
    )
    app.experiments.link_run(human, ws, invalid.id, actual_model=SONNET)
    before = app.predictor.fit(human, ws, HAIKU, samples=0)
    assert before.n == 63
    assert invalid.id not in before.trial_ids


def test_repository_intercept_is_shrunk_toward_the_shared_fit(app, human, ws, seeded):
    beta = _repo_id(app, human, ws, "beta")
    strong = app.predictor.fit(human, ws, HAIKU, repo_penalty=1.0, samples=0)
    weak = app.predictor.fit(human, ws, HAIKU, repo_penalty=1e-6, samples=0)
    offset_strong = abs(_coef_for_repo(strong, beta))
    offset_weak = abs(_coef_for_repo(weak, beta))
    assert offset_strong < offset_weak / 3, (offset_strong, offset_weak)


def test_predict_carries_fit_id_cutoff_and_an_interval(app, human, ws, seeded):
    fit = app.predictor.fit(human, ws, HAIKU, samples=200)
    small = app.predictor.predict(human, ws, HAIKU, "alpha", 16, 2, ["core"])
    large = app.predictor.predict(human, ws, HAIKU, "alpha", 256, 2, ["core"])
    assert small.probability > large.probability
    assert small.inference is True
    assert small.fit_id == fit.id
    assert small.data_cutoff == fit.data_cutoff
    assert small.n == fit.n
    assert small.repository == "alpha"
    assert small.repository_known is True
    assert small.low is not None and small.high is not None
    assert small.low <= small.probability <= small.high


def test_predict_with_an_unseen_repository_uses_the_shared_estimate(app, human, ws, seeded, make_repo):
    app.repositories.register(human, ws, RepositoryRegister(path=str(make_repo("gamma").path)))
    fit = app.predictor.fit(human, ws, HAIKU, samples=0)
    unseen = app.predictor.predict(human, ws, HAIKU, "gamma", 32, 2, ["core"], fit_id=fit.id)
    anonymous = app.predictor.predict(human, ws, HAIKU, None, 32, 2, ["core"], fit_id=fit.id)
    unknown = app.predictor.predict(human, ws, HAIKU, "no-such-repo", 32, 2, ["core"], fit_id=fit.id)
    assert unseen.repository_known is False
    assert unseen.probability == anonymous.probability == unknown.probability
    assert unknown.repository == "no-such-repo"
    assert unseen.low is None and unseen.high is None, "no samples means no interval"


def test_predict_task_reads_the_contract_and_names_missing_fields(app, human, ws, seeded):
    app.predictor.fit(human, ws, HAIKU, samples=0)
    ready = app.tasks.create(
        human,
        ws,
        TaskCreate(
            title="contracted leaf",
            metadata={"contract": {"predicted": {"src_lines": 16, "src_files": 2, "packages": ["core"]}}},
        ),
    )
    app.tasks.link_repository(human, ws, ready.key, "alpha")
    from_task = app.predictor.predict_task(human, ws, HAIKU, ready.key)
    direct = app.predictor.predict(human, ws, HAIKU, "alpha", 16, 2, ["core"])
    assert from_task.probability == direct.probability
    assert from_task.repository == "alpha" and from_task.repository_known is True

    partial = app.tasks.create(
        human,
        ws,
        TaskCreate(
            title="missing files", metadata={"contract": {"predicted": {"src_lines": 16, "packages": []}}}
        ),
    )
    with pytest.raises(ValidationFailed, match="src_files"):
        app.predictor.predict_task(human, ws, HAIKU, partial.key)
    bare = app.tasks.create(human, ws, TaskCreate(title="no contract"))
    with pytest.raises(ValidationFailed, match=r"contract.predicted"):
        app.predictor.predict_task(human, ws, HAIKU, bare.key)


def test_predict_without_a_fit_is_not_found(app, human, ws, seeded):
    assert app.predictor.latest(human, ws, HAIKU) is None
    with pytest.raises(NotFoundError, match="experiment fit --model"):
        app.predictor.predict(human, ws, HAIKU, "alpha", 16, 2, ["core"])


def test_fit_needs_rows_and_both_outcomes(app, human, ws, make_repo):
    with pytest.raises(ValidationFailed, match="at least 4"):
        app.predictor.fit(human, ws, HAIKU)
    app.experiments.create(human, ws, "all-pass", "All pass", None, _protocol())
    app.experiments.freeze(human, ws, "all-pass")
    for lines in (9, 17, 33, 65, 129):
        _measured(app, human, ws, "all-pass", lines=lines, passed=True, repo=None)
    with pytest.raises(ValidationFailed, match="both pass and fail"):
        app.predictor.fit(human, ws, HAIKU)


def test_fit_is_deterministic_for_a_seed(app, human, ws, seeded):
    first = app.predictor.fit(human, ws, HAIKU, samples=20, seed=5)
    second = app.predictor.fit(human, ws, HAIKU, samples=20, seed=5)
    assert first.samples == second.samples
    assert first.coef == second.coef


def test_features_of_clamps_and_counts():
    assert features_of(0, 0, 0) == [0.0, 0.0, 0.0]
    assert features_of(8, 4, 3) == [3.0, 2.0, 3.0]


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


def test_cli_experiment_fit_and_predict(run, home, make_repo):
    run("init")
    run("config", "set", "llm.auto_classify", "false")
    run("config", "set", "llm.auto_embed", "false")
    inst = Workgraph(AppPaths.resolve(home))
    try:
        ctx = RequestContext.human("tester")
        _seed(inst, ctx, None, make_repo)
        leaf = inst.tasks.create(
            ctx,
            None,
            TaskCreate(
                title="contracted leaf",
                metadata={"contract": {"predicted": {"src_lines": 16, "src_files": 2, "packages": ["core"]}}},
            ),
        )
        inst.tasks.link_repository(ctx, None, leaf.key, "alpha")
    finally:
        inst.close()

    fit = run("experiment", "fit", "--model", HAIKU, "--samples", "30")
    assert fit["model"] == HAIKU and fit["n"] > 0 and fit["id"]
    assert fit["data_cutoff"] is not None
    flags = ["--model", HAIKU, "--repo", "alpha", "--lines", "16", "--files", "2", "--packages", "core"]
    out = run("experiment", "predict", *flags, "--fit", fit["id"])
    assert out["inference"] is True
    assert out["fit_id"] == fit["id"]
    assert 0.0 < out["probability"] < 1.0
    assert out["repository_known"] is True
    by_task = run("experiment", "predict", "--model", HAIKU, "--task", leaf.key, "--fit", fit["id"])
    assert by_task["probability"] == out["probability"]
    text = run("experiment", "predict", *flags, json_mode=False)
    assert "inference" in text and "probability" in text and "fit" in text
    assert "predictor fit" in run("experiment", "fit", "--model", HAIKU, "--samples", "0", json_mode=False)
