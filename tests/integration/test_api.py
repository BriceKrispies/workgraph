from __future__ import annotations

import pytest
from starlette.testclient import TestClient

from workgraph.api.app import create_app
from workgraph.api.security import DEVICE_COOKIE, PAIRING_MAX_FAILURES, LanAccess
from workgraph.core.models.schemas import RepositoryRegister


@pytest.fixture
def client(app, ws, monkeypatch):
    monkeypatch.setenv("WORKGRAPH_ALLOWED_HOSTS", "testserver")
    asgi = create_app(app, host="127.0.0.1", port=8765, with_mcp=True)
    token = app.api_token()
    with TestClient(asgi, base_url="http://testserver") as c:
        c.headers["Authorization"] = f"Bearer {token}"
        yield c


def test_health_is_public_but_api_requires_token(app, ws, monkeypatch):
    monkeypatch.setenv("WORKGRAPH_ALLOWED_HOSTS", "testserver")
    asgi = create_app(app, host="127.0.0.1", port=8765, with_mcp=False)
    with TestClient(asgi, base_url="http://testserver") as c:
        assert c.get("/api/v1/health").json()["status"] == "ok"
        r = c.get("/api/v1/workspaces")
        assert r.status_code == 401 and r.json()["error"]["code"] == "unauthorized"
        bad = c.get("/api/v1/workspaces", headers={"Authorization": "Bearer nope"})
        assert bad.status_code == 401


def test_dns_rebinding_and_origin_protection(app, ws):
    asgi = create_app(app, host="127.0.0.1", port=8765, with_mcp=False)
    token = app.api_token()
    with TestClient(asgi, base_url="http://evil.example.com") as c:
        r = c.get("/api/v1/workspaces", headers={"Authorization": f"Bearer {token}"})
        assert r.status_code == 421
    with TestClient(asgi, base_url="http://127.0.0.1:8765") as c:
        headers = {"Authorization": f"Bearer {token}", "Origin": "https://attacker.example"}
        r = c.post("/api/v1/ws/main/tasks", json={"title": "x"}, headers=headers)
        assert r.status_code == 403
        same = c.post(
            "/api/v1/ws/main/tasks",
            json={"title": "x"},
            headers={**headers, "Origin": "http://127.0.0.1:8765"},
        )
        assert same.status_code == 201
        assert "access-control-allow-origin" not in {k.lower() for k in same.headers}


def test_task_lifecycle_over_http(client, app, human, ws, make_repo):
    repo = make_repo("web")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    created = client.post(
        "/api/v1/ws/main/tasks",
        json={"title": "API task", "repositories": ["web"], "status": "ready", "tags": ["api"]},
    )
    assert created.status_code == 201, created.text
    key = created.json()["key"]
    listing = client.get("/api/v1/ws/main/tasks", params={"status": ["ready"], "tag": ["api"]}).json()
    assert listing["total"] == 1
    assert client.post(f"/api/v1/ws/main/tasks/{key}/claim", json={}).json()["status"] == "claimed"
    done = client.post(f"/api/v1/ws/main/tasks/{key}/complete", json={"summary": "done"}).json()
    assert done["status"] == "awaiting_review"
    bad = client.post(f"/api/v1/ws/main/tasks/{key}/accept", json={})
    assert bad.status_code == 422 and bad.json()["error"]["code"] == "validation_failed"
    ok = client.post(f"/api/v1/ws/main/tasks/{key}/accept", json={"manual": True, "reason": "looks good"})
    assert ok.status_code == 200 and ok.json()["mode"] == "manual"
    detail = client.get(f"/api/v1/ws/main/tasks/{key}").json()
    assert detail["status"] == "accepted" and detail["comments"]
    history = client.get(f"/api/v1/ws/main/tasks/{key}/history").json()
    assert next(e["event_type"] for e in history) == "task.created"
    conflict = client.patch(f"/api/v1/ws/main/tasks/{key}", json={"expected_version": 1, "title": "stale"})
    assert conflict.status_code == 409 and conflict.json()["error"]["code"] == "version_conflict"
    invalid = client.post("/api/v1/ws/main/tasks", json={"title": ""})
    assert invalid.status_code == 422
    missing = client.get("/api/v1/ws/main/tasks/WG-999")
    assert missing.status_code == 404


def test_graph_analytics_search_and_settings(client, app, human, ws):
    a = client.post("/api/v1/ws/main/tasks", json={"title": "Fix token crash", "status": "ready"}).json()
    client.post(
        "/api/v1/ws/main/tasks", json={"title": "Follow up", "status": "ready", "depends_on": [a["key"]]}
    )
    graph = client.get("/api/v1/ws/main/graph").json()
    assert len(graph["nodes"]) == 2 and graph["edges"][0]["kind"] == "depends_on"
    overview = client.get("/api/v1/ws/main/analytics/overview").json()
    assert sum(r["value"] for r in overview["created_by_period"]) == 2
    q = client.post(
        "/api/v1/ws/main/analytics/query", json={"metric": "tasks_created", "group_by": ["status"]}
    )
    assert q.status_code == 200 and {r["status"] for r in q.json()["rows"]} == {"ready", "blocked"}
    bad_metric = client.post("/api/v1/ws/main/analytics/query", json={"metric": "nope"})
    assert bad_metric.status_code == 422
    found = client.get("/api/v1/ws/main/search", params={"q": "token"}).json()
    assert found["hits"][0]["key"] == a["key"]
    settings = client.get("/api/v1/settings").json()
    assert settings["config"]["server"]["host"] == "127.0.0.1"
    patched = client.patch("/api/v1/settings", json={"values": {"execution.max_concurrent_runs": 3}})
    assert patched.status_code == 200 and app.config.execution.max_concurrent_runs == 3
    forbidden = client.patch("/api/v1/settings", json={"values": {"agents.evil.executable": "rm"}})
    assert forbidden.status_code == 422
    export = client.post("/api/v1/ws/main/analytics/export", json={"format": "csv", "dataset": "task_facts"})
    assert export.status_code == 200 and b"key" in export.content
    assert client.get("/api/v1/ws/main/events", params={"limit": 5}).json()


def test_graph_filters_by_repository(client, app, human, ws, make_repo):
    for name in ("alpha", "beta"):
        app.repositories.register(human, ws, RepositoryRegister(path=str(make_repo(name).path)))
    a = client.post("/api/v1/ws/main/tasks", json={"title": "In alpha", "repositories": ["alpha"]}).json()
    client.post(
        "/api/v1/ws/main/tasks",
        json={"title": "Also alpha", "repositories": ["alpha"], "depends_on": [a["key"]]},
    )
    client.post("/api/v1/ws/main/tasks", json={"title": "In beta", "repositories": ["beta"]})
    graph = client.get("/api/v1/ws/main/graph", params={"repository": "alpha"}).json()
    assert {n["title"] for n in graph["nodes"]} == {"In alpha", "Also alpha"}
    assert len(graph["edges"]) == 1
    missing = client.get("/api/v1/ws/main/graph", params={"repository": "nope"})
    assert missing.status_code == 404


def test_automation_jobs_and_runs_endpoints(client, app, human, ws):
    rule = {"name": "tagger", "trigger": {"type": "manual"}, "actions": [{"type": "add_tags", "tags": ["x"]}]}
    assert client.post("/api/v1/ws/main/automation/rules", json=rule).status_code == 201
    t = client.post("/api/v1/ws/main/tasks", json={"title": "auto"}).json()
    preview = client.post(
        "/api/v1/ws/main/automation/preview", json={"rule": "tagger", "inputs": {"task": t["key"]}}
    )
    assert preview.json()["status"] == "previewed"
    run = client.post("/api/v1/ws/main/automation/rules/tagger/trigger", json={"inputs": {"task": t["key"]}})
    assert run.json()["status"] == "queued"
    jobs = client.get("/api/v1/jobs").json()
    assert jobs["stats"].get("queued", 0) >= 1
    started = client.post("/api/v1/ws/main/runs", json={"task": t["key"]})
    assert started.status_code == 403  # execution is opt-in


def test_sse_stream_requires_token_and_streams(app, ws, monkeypatch):
    monkeypatch.setenv("WORKGRAPH_ALLOWED_HOSTS", "testserver")
    asgi = create_app(app, host="127.0.0.1", port=8765, with_mcp=False)
    token = app.api_token()
    with TestClient(asgi, base_url="http://testserver") as c:
        assert c.get("/api/v1/ws/main/events/stream").status_code == 401
        with c.stream(
            "GET", "/api/v1/ws/main/events/stream", params={"token": token, "after_seq": 0, "max_events": 2}
        ) as stream:
            assert stream.status_code == 200
            first = next(line for line in stream.iter_lines() if line.startswith("event:"))
            assert first in ("event: domain", "event: queue")


def test_stack_routes(client, app, human, ws, make_repo):
    repo = make_repo("stacked")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    parent = client.post(
        "/api/v1/ws/main/tasks", json={"title": "Parent", "repositories": ["stacked"]}
    ).json()
    client.post(
        "/api/v1/ws/main/tasks",
        json={"title": "Child", "parent": parent["key"], "repositories": ["stacked"]},
    )
    created = client.post("/api/v1/ws/main/stacks", json={"parent": parent["key"]})
    assert created.status_code == 201, created.text
    assert created.json()["repositories"][0]["base_sha"] == repo.head()
    assert [i["state"] for i in created.json()["items"]] == ["pending"]
    assert client.get(f"/api/v1/ws/main/stacks/{parent['key']}").json()["id"] == created.json()["id"]
    assert [s["id"] for s in client.get("/api/v1/ws/main/stacks").json()] == [created.json()["id"]]
    assert client.get("/api/v1/ws/main/stacks/WG-999").status_code == 404
    missing = client.post("/api/v1/ws/main/stacks", json={"parent": parent["key"], "extra": 1})
    assert missing.status_code == 422


def test_stack_item_and_history_routes(client, app, human, ws, make_repo):
    repo = make_repo("stacked-api")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    parent = client.post(
        "/api/v1/ws/main/tasks", json={"title": "Parent", "repositories": ["stacked-api"]}
    ).json()
    client.post(
        "/api/v1/ws/main/tasks",
        json={"title": "Child", "parent": parent["key"], "repositories": ["stacked-api"]},
    )
    stacks = f"/api/v1/ws/main/stacks/{parent['key']}"
    assert client.post("/api/v1/ws/main/stacks", json={"parent": parent["key"]}).status_code == 201
    late = client.post(
        "/api/v1/ws/main/tasks",
        json={"title": "Late", "parent": parent["key"], "repositories": ["stacked-api"]},
    ).json()
    added = client.post(f"{stacks}/items", json={"task": late["key"]})
    assert added.status_code == 201, added.text
    assert [i["task_key"] for i in added.json()["items"]] == ["WG-2", "WG-3"]
    assert [s["parent_key"] for s in client.get(f"/api/v1/ws/main/tasks/{late['key']}/stacks").json()] == [
        parent["key"]
    ]
    assert client.get(f"/api/v1/ws/main/tasks/{parent['key']}/stacks").json() == []
    assert client.post(f"{stacks}/items", json={"task": late["key"], "extra": 1}).status_code == 422
    assert client.post(f"{stacks}/items", json={"task": "WG-2"}).status_code == 409
    removed = client.delete(f"{stacks}/items/{late['key']}")
    assert removed.status_code == 200 and [i["task_key"] for i in removed.json()["items"]] == ["WG-2"]
    assert client.delete(f"{stacks}/items/WG-999").status_code == 404
    shown = client.get(stacks).json()
    assert shown["repositories"][0]["integration_tip"] is None
    history = client.get(f"{stacks}/history")
    assert history.status_code == 200
    assert [e["event_type"] for e in history.json()] == [
        "stack.created",
        "stack.item_added",
        "stack.item_removed",
    ]
    assert client.get("/api/v1/ws/main/stacks/WG-999/history").status_code == 404


def test_spa_fallback_serves_ui_shell(client):
    page = client.get("/tasks/WG-1")
    assert page.status_code == 200 and "Workgraph" in page.text
    assert client.get("/api/v1/does-not-exist").status_code == 404


def _lan_client(asgi, ip: str, base_url: str = "http://192.168.1.20:8765") -> TestClient:
    return TestClient(asgi, base_url=base_url, client=(ip, 50000), follow_redirects=False)


def test_lan_mode_requires_pairing_before_handing_out_the_token(app, ws):
    lan = LanAccess.build("192.168.1.20")
    asgi = create_app(app, host="127.0.0.1", port=8765, with_mcp=True, lan=lan)
    token = app.api_token()
    # Not entered as a context manager: the MCP lifespan may only run once per app.
    assert token in _lan_client(asgi, "127.0.0.1", "http://127.0.0.1:8765").get("/").text
    with _lan_client(asgi, "192.168.1.42") as phone:
        page = phone.get("/tasks")
        assert page.status_code == 303 and page.headers["location"] == "/pair"
        assert token not in phone.get("/pair").text
        assert phone.get("/api/v1/workspaces").status_code == 401
        wrong = phone.post("/pair", data={"code": "AAAA-AAAA"})
        assert wrong.status_code == 403 and DEVICE_COOKIE not in wrong.cookies
        ok = phone.post("/pair", data={"code": lan.pairing_code.lower()})
        assert ok.status_code == 303 and DEVICE_COOKIE in ok.cookies
        assert token in phone.get("/").text
        auth = {"Authorization": f"Bearer {token}"}
        assert phone.get("/api/v1/workspaces", headers=auth).status_code == 200
        # MCP stays loopback-only even with the token.
        assert phone.post("/mcp/", headers=auth, json={}).status_code == 403


def test_lan_mode_rejects_public_clients_and_locks_pairing(app, ws):
    lan = LanAccess.build("192.168.1.20")
    asgi = create_app(app, host="127.0.0.1", port=8765, with_mcp=False, lan=lan)
    with _lan_client(asgi, "203.0.113.9") as outsider:
        r = outsider.get("/")
        assert r.status_code == 403 and r.json()["error"]["code"] == "forbidden_client"
    with _lan_client(asgi, "192.168.1.42") as phone:
        for _ in range(PAIRING_MAX_FAILURES):
            phone.post("/pair", data={"code": "WRONG"})
        assert phone.post("/pair", data={"code": lan.pairing_code}).status_code == 429


def test_lan_networks_narrow_the_allowed_clients():
    lan = LanAccess.build("192.168.1.20", ["192.168.1.0/24"])
    assert lan.client_allowed("192.168.1.7") and lan.client_allowed("127.0.0.1")
    assert not lan.client_allowed("10.0.0.5") and not lan.client_allowed("testclient")
