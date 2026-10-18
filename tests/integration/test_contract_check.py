from __future__ import annotations

import json
from typing import Any

import pytest
from typer.testing import CliRunner

from tests.integration.test_predictor import HAIKU, _seed
from workgraph.app import Workgraph
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.config.paths import AppPaths
from workgraph.core.context import RequestContext
from workgraph.core.errors import ValidationFailed
from workgraph.core.models.schemas import RepositoryRegister, TaskCreate

TARGET = 0.8


def _contract(**overrides: Any) -> dict[str, Any]:
    contract: dict[str, Any] = {
        "files": ["src/pkg/mod.py", "tests/test_mod.py"],
        "oracle": [["pytest", "-q", "tests/test_mod.py"]],
        "gates": [["ruff", "check", "."]],
        "out_of_scope": ["the CLI"],
        "checks": ["tests/test_mod.py::test_f"],
        "predicted": {"src_lines": 16, "src_files": 2, "packages": ["core"]},
    }
    contract.update(overrides)
    return contract


def _contracted(
    app: Workgraph,
    human: RequestContext,
    ws: str | None,
    *,
    title: str = "contracted leaf",
    contract: dict[str, Any] | None = None,
    tags: list[str] | None = None,
    repo: str | None = "alpha",
) -> Any:
    task = app.tasks.create(
        human,
        ws,
        TaskCreate(
            title=title,
            acceptance_criteria=["the function returns one"],
            tags=tags or [],
            metadata={"contract": _contract() if contract is None else contract},
        ),
    )
    if repo is not None:
        app.tasks.link_repository(human, ws, task.key, repo)
    return task


def _check(app: Workgraph, human: RequestContext, ws: str | None, key: str, **kwargs: Any) -> Any:
    return app.contract_checker.check(human, ws, key, model=HAIKU, target=TARGET, **kwargs)


@pytest.fixture
def fitted(app, human, ws, make_repo):
    _seed(app, human, ws, make_repo)
    return app.predictor.fit(human, ws, HAIKU, samples=0)


def test_small_complete_contract_is_a_leaf(app, human, ws, fitted):
    task = _contracted(app, human, ws)
    verdict = _check(app, human, ws, task.key)
    assert verdict.verdict == "leaf"
    assert verdict.missing == ()
    assert verdict.prediction is not None
    assert verdict.prediction.probability >= TARGET
    assert verdict.prediction.fit_id == fitted.id
    assert verdict.prediction.inference is True
    assert verdict.prediction.repository_known is True
    assert verdict.model == HAIKU and verdict.target == TARGET


def test_large_complete_contract_is_split(app, human, ws, fitted):
    big = {"src_lines": 256, "src_files": 2, "packages": ["core"]}
    task = _contracted(app, human, ws, contract=_contract(predicted=big))
    verdict = _check(app, human, ws, task.key)
    assert verdict.verdict == "split"
    assert verdict.prediction is not None and verdict.prediction.probability < TARGET
    assert any("below the target" in reason for reason in verdict.reasons)


def test_missing_contract_fields_are_incomplete(app, human, ws, fitted):
    no_oracle = {key: value for key, value in _contract().items() if key != "oracle"}
    task = _contracted(app, human, ws, contract=no_oracle)
    verdict = _check(app, human, ws, task.key)
    assert verdict.verdict == "incomplete"
    assert verdict.missing == ("contract.oracle",)
    assert verdict.prediction is None


def test_needs_decision_is_not_delegable_even_with_a_complete_contract(app, human, ws, fitted):
    task = _contracted(app, human, ws, tags=["needs-decision"])
    verdict = _check(app, human, ws, task.key)
    assert verdict.verdict == "not_delegable"
    assert any("needs-decision" in reason for reason in verdict.reasons)
    assert verdict.prediction is None


def test_no_fit_for_the_model_is_incomplete_naming_the_fit_command(app, human, ws, make_repo):
    _seed(app, human, ws, make_repo)
    task = _contracted(app, human, ws)
    verdict = app.contract_checker.check(human, ws, task.key, model=HAIKU)
    assert verdict.verdict == "incomplete"
    assert verdict.prediction is None
    assert f"workgraph experiment fit --model {HAIKU}" in verdict.reasons[0]


def test_leaf_in_an_unseen_repository_names_the_calibration(app, human, ws, fitted, make_repo):
    app.repositories.register(human, ws, RepositoryRegister(path=str(make_repo("gamma").path)))
    task = _contracted(app, human, ws, repo="gamma")
    verdict = _check(app, human, ws, task.key)
    assert verdict.verdict == "leaf"
    assert verdict.prediction is not None and verdict.prediction.repository_known is False
    assert "repository not seen by the fit: run a replay calibration first" in verdict.reasons


def test_invalid_target_is_rejected(app, human, ws, fitted):
    task = _contracted(app, human, ws)
    with pytest.raises(ValidationFailed, match="target"):
        app.contract_checker.check(human, ws, task.key, model=HAIKU, target=1.5)


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


def test_cli_contract_check_json_and_text(run, home, make_repo):
    run("init")
    run("config", "set", "llm.auto_classify", "false")
    run("config", "set", "llm.auto_embed", "false")
    inst = Workgraph(AppPaths.resolve(home))
    try:
        ctx = RequestContext.human("tester")
        _seed(inst, ctx, None, make_repo)
        leaf = _contracted(inst, ctx, None)
    finally:
        inst.close()
    fit = run("experiment", "fit", "--model", HAIKU, "--samples", "0")

    out = run("task", "contract-check", leaf.key, "--model", HAIKU)
    assert out["verdict"] == "leaf"
    assert out["model"] == HAIKU and out["target"] == TARGET
    assert out["prediction"]["fit_id"] == fit["id"]
    assert out["prediction"]["inference"] is True
    assert out["missing"] == [] and out["reasons"]

    text = run("task", "contract-check", leaf.key, "--model", HAIKU, "--target", "0.99", json_mode=False)
    assert "split" in text and "inference" in text and fit["id"] in text
