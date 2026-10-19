"""The real Jira client, mapper and sync together, over a mocked Jira Cloud HTTP API."""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import httpx
from sqlalchemy import select

from workgraph.app import Workgraph
from workgraph.config.settings import JiraTrackerConfig
from workgraph.core.context import RequestContext
from workgraph.core.models.schemas import RepositoryRegister
from workgraph.integrations.trackers.jira import make_jira_client
from workgraph.integrations.trackers.sync import TrackerSyncService
from workgraph.persistence.orm import Task, TaskAcceptance, TaskLink, TaskRepository

FIXTURES = Path(__file__).parent.parent / "fixtures" / "jira"
CONNECTION = "work-jira"


def fixture(name: str) -> dict[str, Any]:
    return json.loads((FIXTURES / f"{name}.json").read_text(encoding="utf-8"))


def jira_cloud(requests: list[httpx.Request]) -> httpx.MockTransport:
    issues = [fixture("cloud_story"), fixture("cloud_done_wont_do")]

    def handler(request: httpx.Request) -> httpx.Response:
        requests.append(request)
        path = request.url.path
        if path == "/rest/api/3/myself":
            return httpx.Response(200, json={"timeZone": "UTC"})
        if path == "/rest/api/3/search/jql":
            if request.url.params.get("fields") == "id":
                return httpx.Response(
                    200, json={"issues": [{"id": i["id"], "key": i["key"]} for i in issues], "isLast": True}
                )
            return httpx.Response(200, json={"issues": issues, "isLast": True})
        if path == "/rest/dev-status/latest/issue/detail":
            if request.url.params.get("issueId") == "10001":
                return httpx.Response(200, json=fixture("dev_status"))
            return httpx.Response(200, json={"detail": []})
        # Parents and blockers outside the fixtures do not exist.
        return httpx.Response(404, json={"errorMessages": ["Issue does not exist"]})

    return httpx.MockTransport(handler)


def test_cloud_issues_become_tasks_with_pull_requests_and_status(
    app: Workgraph, ws: str, human: RequestContext, make_repo: Callable[..., Any]
) -> None:
    config = app.config
    config.trackers[CONNECTION] = JiraTrackerConfig(
        base_url="https://acme.atlassian.net",
        workspace=ws,
        projects=["ABC"],
        email_env="JIRA_EMAIL",
        token_env="JIRA_TOKEN",
        pr_fields=["customfield_10042"],
    )
    app.config_store.save_global(config)
    repo = make_repo("svc-billing")
    repo.git("remote", "add", "origin", "git@github.com:acme/svc-billing.git")
    billing = app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path))).id

    requests: list[httpx.Request] = []
    transport = jira_cloud(requests)
    env = {"JIRA_EMAIL": "me@example.com", "JIRA_TOKEN": "secret"}
    service = TrackerSyncService(
        app, client_factory=lambda cfg: make_jira_client(cfg, env=env, transport=transport)
    )

    report = service.sync(RequestContext.system("cli"), CONNECTION, full=True)

    assert report.created == 2, report.as_dict()
    assert {r.method for r in requests} == {"GET"}
    with app.db.session() as session:
        story = session.scalar(select(Task).where(Task.key == "ABC-123"))
        done = session.scalar(select(Task).where(Task.key == "ABC-150"))
        assert story is not None and done is not None
        assert story.priority == "high"
        assert story.status == "backlog"
        assert done.status == "cancelled"
        links = {
            link.normalized_url: link
            for link in session.scalars(select(TaskLink).where(TaskLink.task_id == story.id))
        }
        billing_pr = links["github.com/acme/svc-billing/pull/42"]
        assert billing_pr.repository_id == billing
        assert billing_pr.sources == ["custom_field", "dev_panel"]
        assert links["github.com/acme/svc-api/pull/43"].repository_id is None
        repos = set(
            session.scalars(select(TaskRepository.repository_id).where(TaskRepository.task_id == story.id))
        )
        assert repos == {billing}
        assert session.scalar(select(TaskAcceptance).where(TaskAcceptance.task_id == done.id)) is None
    assert any("svc-api" in url for url in report.unresolved_pull_requests)
