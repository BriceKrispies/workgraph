from __future__ import annotations

import pytest

from workgraph.app import Workgraph
from workgraph.config.paths import AppPaths
from workgraph.core.context import RequestContext
from workgraph.demo.scenario import run_demo
from workgraph.demo.seed import seed_demo


@pytest.mark.e2e
def test_required_end_to_end_demonstration(tmp_path):
    report = run_demo(tmp_path / "demo-home", log=lambda _m: None)
    assert report.ok
    assert [s["step"] for s in report.steps] == list(range(1, 26))
    by_step = {s["step"]: s for s in report.steps}
    churn = by_step[19]["cumulative_churn"]
    final = by_step[19]["final_change_surface"]
    assert churn["insertions"] > final["insertions"]
    assert by_step[25]["before"] == by_step[25]["after"]


@pytest.mark.e2e
def test_demo_seed_populates_real_data(tmp_path):
    app = Workgraph(AppPaths.resolve(tmp_path / "seed-home"))
    try:
        summary = seed_demo(app, "demo", log=lambda _m: None)
        assert summary["tasks"] >= 9 and summary["attempts"] >= 7 and summary["commits"] >= 5
        ctx = RequestContext.human("t")
        overview = app.analytics.overview(ctx, "demo")
        assert sum(r["value"] for r in overview["accepted_by_period"]) >= 3
        assert app.automation.notifications(ctx, "demo")
    finally:
        app.close()
