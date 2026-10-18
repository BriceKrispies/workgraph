from __future__ import annotations

import os
import sys

import httpx2
import pytest
from mcp import Client
from mcp.client.stdio import StdioServerParameters
from mcp.client.streamable_http import streamable_http_client

from workgraph.core.models.schemas import RepositoryRegister
from workgraph.demo.scenario import _free_port, _start_api

REQUIRED_TOOLS = {
    "workspace_list",
    "workspace_get",
    "project_list",
    "project_get",
    "repository_list",
    "repository_get",
    "repository_register",
    "repository_status",
    "task_create",
    "task_get",
    "task_search",
    "task_list",
    "task_update",
    "task_claim",
    "task_release",
    "task_complete",
    "task_get_ready",
    "task_add_dependency",
    "task_remove_dependency",
    "task_add_relation",
    "task_add_comment",
    "task_get_history",
    "task_get_attempts",
    "task_link_repository",
    "stack_get",
    "stack_list",
    "stack_history",
    "stack_create",
    "stack_add_item",
    "stack_remove_item",
    "stack_integrate",
    "stack_verify",
    "task_link_commit",
    "task_report_progress",
    "task_report_result",
    "attempt_create",
    "attempt_get",
    "attempt_list",
    "run_start",
    "run_get",
    "run_cancel",
    "git_get_changes",
    "git_get_task_diff",
    "classification_get",
    "classification_request",
    "analytics_query",
    "analytics_report",
    "automation_list",
    "automation_trigger",
}


@pytest.mark.slow
async def test_stdio_server_discovery_invocation_and_persistence(app, human, ws, home, make_repo):
    repo = make_repo("mcp-repo")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))
    params = StdioServerParameters(
        command=sys.executable,
        args=["-m", "workgraph", "mcp", "--agent-name", "tester"],
        env={**os.environ, "WORKGRAPH_HOME": str(home)},
    )
    async with Client(params, read_timeout_seconds=60) as client:
        tools = {t.name: t for t in (await client.list_tools()).tools}
        assert set(tools) >= REQUIRED_TOOLS
        create_schema = tools["task_create"].input_schema
        assert "title" in create_schema["required"]
        assert tools["task_create"].output_schema is not None
        created = await client.call_tool(
            "task_create", {"title": "From MCP", "repositories": ["mcp-repo"], "ready": True}
        )
        assert not created.is_error
        key = created.structured_content["key"]
        ready = await client.call_tool("task_get_ready", {})
        assert [t["key"] for t in ready.structured_content["tasks"]] == [key]
        claimed = await client.call_tool("task_claim", {"task": key})
        assert claimed.structured_content["claimed_by"] == "agent:tester"
        attempt = await client.call_tool("attempt_create", {"task": key, "model": "test-model"})
        attempt_id = attempt.structured_content["id"]
        sha = repo.commit(f"mcp work\n\nTask-ID: {key}", {"m.py": "x = 1\n"})
        linked = await client.call_tool("task_link_commit", {"task": key, "sha": sha})
        assert linked.structured_content["links"][0]["source"] == "explicit"
        await client.call_tool(
            "task_report_progress",
            {"task": key, "message": "halfway", "percent": 50, "attempt_id": attempt_id},
        )
        result = await client.call_tool(
            "task_report_result",
            {
                "task": key,
                "outcome": "succeeded",
                "summary": "done",
                "attempt_id": attempt_id,
                "input_tokens": 10,
            },
        )
        assert result.structured_content["status"] == "awaiting_review"
        # agents cannot accept work or run arbitrary execution
        denied = await client.call_tool("run_start", {"task": key})
        assert denied.is_error and "permission" in str(denied.content).lower()
        missing = await client.call_tool("task_get", {"task": "WG-999"})
        assert missing.is_error and "not_found" in str(missing.content)
        invalid = await client.call_tool("task_create", {"title": ""})
        assert invalid.is_error
        resources = await client.list_resource_templates()
        assert any("tasks/{task}" in r.uri_template for r in resources.resource_templates)
        resource = await client.read_resource(f"workgraph://workspaces/main/tasks/{key}")
        content = resource.contents[0]
        assert key in getattr(content, "text", "")
        catalog = await client.call_tool("analytics_report", {"report": "catalog"})
        assert any(m["id"] == "cumulative_churn" for m in catalog.structured_content["metrics"])
    # persisted, visible to the application
    task = app.tasks.get(human, ws, key)
    assert task.status == "awaiting_review"
    attempt_out = app.attempts.get(human, attempt_id)
    assert attempt_out.status == "succeeded" and attempt_out.usage == {"input_tokens": 10}
    assert attempt_out.usage_source == "agent_reported"
    assert app.git.task_commits(human, ws, key)[0]["sha"] == sha
    actors = {e.actor for e in app.history.task_history(human, ws, key)}
    assert "agent:tester" in actors


@pytest.mark.slow
async def test_stack_mutations_need_stack_write(app, human, ws, home, make_repo):
    repo = make_repo("stack-mcp")
    app.repositories.register(human, ws, RepositoryRegister(path=str(repo.path)))

    def stdio(*grants: str) -> StdioServerParameters:
        args = ["-m", "workgraph", "mcp", "--agent-name", "tester"]
        for grant in grants:
            args += ["--grant", grant]
        return StdioServerParameters(
            command=sys.executable, args=args, env={**os.environ, "WORKGRAPH_HOME": str(home)}
        )

    async with Client(stdio(), read_timeout_seconds=60) as client:
        parent = await client.call_tool("task_create", {"title": "Stacked", "repositories": ["stack-mcp"]})
        child = await client.call_tool(
            "task_create",
            {"title": "Change", "parent": parent.structured_content["key"], "repositories": ["stack-mcp"]},
        )
        parent_key = parent.structured_content["key"]
        child_key = child.structured_content["key"]
        denied = await client.call_tool("stack_create", {"parent": parent_key})
        assert denied.is_error and "stack:write" in str(denied.content)
        assert not (await client.call_tool("stack_list", {})).is_error
        assert (await client.call_tool("stack_get", {"parent": parent_key})).is_error
    async with Client(stdio("stack:write"), read_timeout_seconds=60) as client:
        created = await client.call_tool("stack_create", {"parent": parent_key})
        assert not created.is_error
        assert created.structured_content["parent_key"] == parent_key
        assert [i["task_key"] for i in created.structured_content["items"]] == [child_key]
        extra = await client.call_tool(
            "task_create", {"title": "Late", "parent": parent_key, "repositories": ["stack-mcp"]}
        )
        late_key = extra.structured_content["key"]
        added = await client.call_tool("stack_add_item", {"parent": parent_key, "task": late_key})
        assert [i["task_key"] for i in added.structured_content["items"]] == [child_key, late_key]
        removed = await client.call_tool("stack_remove_item", {"parent": parent_key, "task": late_key})
        assert [i["task_key"] for i in removed.structured_content["items"]] == [child_key]
        history = await client.call_tool("stack_history", {"parent": parent_key})
        assert [e["event_type"] for e in history.structured_content["events"]] == [
            "stack.created",
            "stack.item_added",
            "stack.item_removed",
        ]
        integrated = await client.call_tool("stack_integrate", {"parent": parent_key})
        assert (
            not integrated.is_error and integrated.structured_content["jobs"][0]["repository"] == "stack-mcp"
        )
        verify_missing = await client.call_tool("stack_verify", {"parent": "WG-999"})
        assert verify_missing.is_error and "not_found" in str(verify_missing.content)
    assert app.stacks.get(human, ws, parent_key).items[0].task_key == child_key


@pytest.mark.slow
async def test_streamable_http_transport(app, human, ws):
    port = _free_port()
    thread, server = _start_api(app, port)
    try:
        token = app.api_token()
        url = f"http://127.0.0.1:{port}/mcp/"
        async with (
            httpx2.AsyncClient(headers={"Authorization": f"Bearer {token}"}, timeout=30) as http,
            Client(streamable_http_client(url, http_client=http), read_timeout_seconds=30) as client,
        ):
            names = {t.name for t in (await client.list_tools()).tools}
            assert names >= REQUIRED_TOOLS
            created = await client.call_tool("task_create", {"title": "Over HTTP"})
            assert created.structured_content["key"] == "WG-1"
        async with httpx2.AsyncClient(timeout=10) as anon:
            response = await anon.post(url, json={"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
            assert response.status_code == 401
    finally:
        server.should_exit = True
        thread.join(timeout=30)
    assert app.tasks.get(human, ws, "WG-1").title == "Over HTTP"
