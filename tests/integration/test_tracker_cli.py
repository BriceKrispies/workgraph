from __future__ import annotations

import json

import pytest
from typer.testing import CliRunner

from workgraph.app import Workgraph
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.config.settings import JiraTrackerConfig
from workgraph.integrations.trackers.sync import TrackerSyncService
from workgraph.integrations.trackers.types import SyncReport


def _configure(app: Workgraph) -> None:
    config = app.config
    config.trackers["work-jira"] = JiraTrackerConfig(
        base_url="https://acme.atlassian.net",
        workspace="work",
        projects=["ABC"],
        email_env="WG_TEST_JIRA_EMAIL",
        token_env="WG_TEST_JIRA_TOKEN",
    )
    app.config_store.save_global(config)


@pytest.fixture
def run(app, home, monkeypatch):
    _configure(app)
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
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


def test_tracker_list(run):
    listing = run("tracker", "list")
    assert listing == [
        {
            "name": "work-jira",
            "flavour": "cloud",
            "base_url": "https://acme.atlassian.net",
            "workspace": "work",
            "projects": ["ABC"],
        }
    ]
    assert "work-jira" in run("tracker", "list", json_mode=False)


def test_tracker_sync_runs_now(run, monkeypatch):
    calls: list[tuple[str, bool]] = []

    def fake_sync(self, ctx, connection, *, full=False):
        self.connection(connection)
        calls.append((connection, full))
        return SyncReport(connection=connection, full=full, fetched=2, created=2)

    monkeypatch.setattr(TrackerSyncService, "sync", fake_sync)
    report = run("tracker", "sync", "work-jira")
    assert report["connection"] == "work-jira" and report["full"] is False
    assert (report["fetched"], report["created"], report["errors"]) == (2, 2, [])
    assert run("tracker", "sync", "work-jira", "--full")["full"] is True
    assert calls == [("work-jira", False), ("work-jira", True)]
    assert "work-jira" in run("tracker", "sync", "work-jira", json_mode=False)


def test_tracker_sync_queue_returns_job_id(run, app):
    queued = run("tracker", "sync", "work-jira", "--queue", "--full")
    job = app.queue.get(queued["job_id"])
    assert job.kind == "tracker_sync"
    assert job.payload == {"connection": "work-jira", "full": True}
    assert job.resource_keys == ["tracker:work-jira"]
    again = run("tracker", "sync", "work-jira", "--queue", "--full")
    assert again["job_id"] == queued["job_id"]


def test_tracker_sync_unknown_connection(run, monkeypatch):
    def fake_sync(self, ctx, connection, *, full=False):
        self.connection(connection)
        raise AssertionError("unreachable for an unknown connection")

    monkeypatch.setattr(TrackerSyncService, "sync", fake_sync)
    run("tracker", "sync", "nope", expect=4)
    run("tracker", "sync", "nope", "--queue", expect=4)


def test_tracker_status(run, monkeypatch):
    def fake_status(self, ctx, connection):
        self.connection(connection)
        return {"connection": connection, "cursor": None, "tracked_tasks": 0, "unresolved_pull_requests": []}

    monkeypatch.setattr(TrackerSyncService, "status", fake_status)
    one = run("tracker", "status", "work-jira")
    assert one["connection"] == "work-jira"
    everything = run("tracker", "status")
    assert [s["connection"] for s in everything] == ["work-jira"]
    assert "work-jira" in run("tracker", "status", json_mode=False)
    run("tracker", "status", "nope", expect=4)
