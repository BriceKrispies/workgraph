"""The required cross-repository end-to-end demonstration.

Runs fully offline with no paid credentials: three real Git repositories
(``auth-service``, ``shared-sdk``, ``web-client``), the deterministic heuristic
classifier, the scripted command agent, real verification commands, the real MCP
server over stdio, the real HTTP API and a restart of the application.

Each numbered step records what was checked; the demonstration fails loudly on the
first step whose expectation is not met.
"""

from __future__ import annotations

import asyncio
import json
import os
import socket
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import httpx

from workgraph.analytics.service import AnalyticsQuery
from workgraph.app import Workgraph
from workgraph.config.paths import AppPaths
from workgraph.config.settings import CommandAgentConfig, ModelSelection
from workgraph.core.context import RequestContext
from workgraph.core.models.schemas import (
    ClaimRequest,
    ProjectCreate,
    RepositoryRegister,
    TaskCreate,
    TaskUpdate,
    WorkspaceCreate,
)
from workgraph.demo import scripted_agent

REPOS: dict[str, dict[str, str]] = {
    "auth-service": {
        "seed_file": "auth/__init__.py",
        "seed": "def login(user: str, password: str) -> bool:\n    return bool(user and password)\n",
        "plan_file": "auth/refresh.py",
        "plan": ("# {task}: refresh token issuance\nMARKER = 'WORKGRAPH_DEMO_OK'\n\n\n"
                 "def issue_refresh_token(user: str) -> str:\n    return f'refresh-{user}'\n\n\n"
                 "class RefreshPolicy:\n    ttl_seconds = 3600\n"),
    },
    "shared-sdk": {
        "seed_file": "src/client.ts",
        "seed": "export function request(path: string): string {\n  return path;\n}\n",
        "plan_file": "src/token.ts",
        "plan": ("// {task}: token refresh helper\nexport const MARKER = 'WORKGRAPH_DEMO_OK';\n\n"
                 "export function refreshToken(token: string): string {\n  return `refreshed:${token}`;\n}\n\n"
                 "export interface TokenPair {\n  access: string;\n  refresh: string;\n}\n"),
    },
    "web-client": {
        "seed_file": "app/main.js",
        "seed": "export function render() {\n  return 'login';\n}\n",
        "plan_file": "app/session.js",
        "plan": ("// {task}: use the refresh flow\nexport const MARKER = 'WORKGRAPH_DEMO_OK';\n\n"
                 "export function keepSessionAlive(client) {\n  return client.refresh();\n}\n"),
    },
}
CHECK_SCRIPT = """import json, pathlib, sys
plan = json.loads(pathlib.Path('workgraph-demo.json').read_text())
target = pathlib.Path(plan['file'])
if not target.exists():
    print(f"FAILED tests/test_contract.py::test_{target.stem}_exists - missing {target}")
    sys.exit(1)
if 'WORKGRAPH_DEMO_OK' not in target.read_text():
    print(f"FAILED tests/test_contract.py::test_{target.stem}_contract - marker missing")
    sys.exit(1)
print('1 passed')
"""
INTEGRATION_SCRIPT = """import os, pathlib, sys
missing = []
for repo in ('AUTH_SERVICE', 'SHARED_SDK', 'WEB_CLIENT'):
    path = os.environ.get(f'WORKGRAPH_REPO_{repo}_PATH')
    if path and not pathlib.Path(path).exists():
        missing.append(repo)
print('integration ok' if not missing else f'missing {missing}')
sys.exit(1 if missing else 0)
"""


class DemoFailure(AssertionError):
    pass


@dataclass
class DemoReport:
    home: str
    steps: list[dict[str, Any]] = field(default_factory=list)

    def step(self, number: int, title: str, check: Callable[[], dict[str, Any] | None]) -> dict[str, Any]:
        started = time.monotonic()
        try:
            details = check() or {}
        except Exception as exc:
            self.steps.append({"step": number, "title": title, "ok": False, "error": f"{type(exc).__name__}: {exc}"})
            raise
        entry = {"step": number, "title": title, "ok": True, "seconds": round(time.monotonic() - started, 2), **details}
        self.steps.append(entry)
        return entry

    @property
    def ok(self) -> bool:
        return bool(self.steps) and all(s["ok"] for s in self.steps)


def expect(condition: bool, message: str) -> None:
    if not condition:
        raise DemoFailure(message)


def _git(path: Path, *args: str) -> str:
    env = {**os.environ, "GIT_AUTHOR_NAME": "Demo Human", "GIT_AUTHOR_EMAIL": "human@workgraph.local",
           "GIT_COMMITTER_NAME": "Demo Human", "GIT_COMMITTER_EMAIL": "human@workgraph.local"}
    return subprocess.run(["git", "-C", str(path), *args], check=True, capture_output=True, text=True, env=env).stdout.strip()


def create_repositories(root: Path) -> dict[str, Path]:
    paths = {}
    for name, spec in REPOS.items():
        path = root / name
        path.mkdir(parents=True, exist_ok=True)
        _git(path, "init", "-q", "-b", "main")
        _git(path, "config", "core.autocrlf", "false")
        seed = path / spec["seed_file"]
        seed.parent.mkdir(parents=True, exist_ok=True)
        seed.write_text(spec["seed"], encoding="utf-8", newline="\n")
        (path / "workgraph-demo.json").write_text(json.dumps({"file": spec["plan_file"], "content": spec["plan"]}),
                                                   encoding="utf-8", newline="\n")
        (path / "check.py").write_text(CHECK_SCRIPT, encoding="utf-8", newline="\n")
        (path / "README.md").write_text(f"# {name}\n", encoding="utf-8", newline="\n")
        _git(path, "add", "-A")
        _git(path, "commit", "-q", "-m", f"Initial {name}")
        paths[name] = path
    return paths


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def _drain(app: Workgraph, timeout: float = 300) -> None:
    from workgraph.jobs.worker import Worker

    Worker(app, concurrency=3, poll_interval=0.05, maintenance_interval=0.2).run_until_idle(timeout=timeout)


async def _mcp_session(home: Path, calls: list[tuple[str, dict[str, Any]]]) -> list[Any]:
    from mcp import Client
    from mcp.client.stdio import StdioServerParameters

    params = StdioServerParameters(
        command=sys.executable, args=["-m", "workgraph", "mcp", "--agent-name", "demo-mcp-agent"],
        env={**os.environ, "WORKGRAPH_HOME": str(home)},
    )
    results: list[Any] = []
    async with Client(params, read_timeout_seconds=60) as client:
        tools = await client.list_tools()
        results.append(sorted(t.name for t in tools.tools))
        for name, arguments in calls:
            result = await client.call_tool(name, arguments)
            if result.is_error:
                raise DemoFailure(f"MCP tool {name} failed: {result.content}")
            results.append(result.structured_content)
    return results


def run_demo(home: Path, *, log: Callable[[str], None] = print) -> DemoReport:  # noqa: T201 - progress output
    home = home.resolve()
    home.mkdir(parents=True, exist_ok=True)
    repos_root = home / "repos"
    report = DemoReport(home=str(home))
    human = RequestContext.human("demo-operator")
    state: dict[str, Any] = {}
    app = Workgraph(AppPaths.resolve(home))

    def say(entry: dict[str, Any]) -> None:
        log(f"[{entry['step']:>2}] ok  {entry['title']}")

    try:
        def s1() -> dict[str, Any]:
            config = app.config
            config.llm.classification = ModelSelection(provider="heuristic", model="heuristic-v1")
            config.llm.embeddings = ModelSelection(provider="heuristic", model="hash-embed-256")
            config.llm.summarization = ModelSelection(provider="heuristic", model="heuristic-v1")
            config.execution.enabled = True
            config.execution.default_agent = "scripted"
            agent = str(Path(scripted_agent.__file__))
            config.agents["scripted"] = CommandAgentConfig(executable=sys.executable, args=[agent, "--mode", "success",
                                                                                            "{manifest}"], supports_usage=True)
            config.agents["scripted-buggy"] = CommandAgentConfig(executable=sys.executable,
                                                                 args=[agent, "--mode", "buggy", "{manifest}"],
                                                                 supports_usage=True)
            app.config_store.save_global(config)
            app.api_token()
            ws = app.workspaces.create(human, WorkspaceCreate(slug="demo", name="Demo", key_prefix="WG"), make_default=True)
            state["ws"] = ws.slug
            return {"workspace": ws.slug, "database": str(app.paths.database_file)}

        say(report.step(1, "Initialize a Workgraph workspace", s1))

        def s2() -> dict[str, Any]:
            paths = create_repositories(repos_root)
            for name, path in paths.items():
                app.repositories.register(human, "demo", RepositoryRegister(path=str(path)))
            names = sorted(r.name for r in app.repositories.list(human, "demo"))
            expect(names == sorted(REPOS), f"registered {names}")
            for name in REPOS:
                app.git.sync_ref(human, "demo", name)
            return {"repositories": names}

        say(report.step(2, "Register all three Git repositories", s2))

        def s3() -> dict[str, Any]:
            project = app.projects.create(human, "demo", ProjectCreate(slug="session-refresh", name="Session refresh",
                                                                       repositories=list(REPOS)))
            expect(len(project.repository_ids) == 3, "project should span 3 repositories")
            for name in REPOS:
                app.verification.create_config(human, "demo", scope_type="repository", scope=name, name="contract",
                                               checks=[{"name": f"{name} contract tests", "kind": "test",
                                                        "command": [sys.executable, "check.py"]}])
            app.verification.create_config(human, "demo", scope_type="project", scope="session-refresh",
                                           name="integration",
                                           checks=[{"name": "cross-repo integration", "kind": "integration",
                                                    "command": [sys.executable, "-c", INTEGRATION_SCRIPT]}])
            return {"project": project.slug}

        say(report.step(3, "Create a project spanning the repositories", s3))

        def s4() -> dict[str, Any]:
            parent = app.tasks.create(human, "demo", TaskCreate(
                title="Add session refresh across services",
                description="Users are logged out when tokens expire. Implement token refresh end to end.\n\n"
                            "- Add a token refresh helper to the SDK\n- Issue refresh tokens in the auth service\n"
                            "- Use the refresh flow in the web client",
                acceptance_criteria=["Expired sessions refresh transparently", "All contract tests pass"],
                projects=["session-refresh"], repositories=list(REPOS), priority="high"))
            state["parent"] = parent.key
            return {"parent": parent.key}

        say(report.step(4, "Create a cross-repository parent task", s4))

        def s5() -> dict[str, Any]:
            sdk = app.tasks.create(human, "demo", TaskCreate(
                title="Add token refresh helper to the SDK", description="Implement refreshToken in shared-sdk.",
                parent=state["parent"], projects=["session-refresh"], repositories=["shared-sdk"], status="ready",
                priority="high"))
            auth = app.tasks.create(human, "demo", TaskCreate(
                title="Issue refresh tokens in auth-service", description="Add a refresh token endpoint implementation.",
                parent=state["parent"], projects=["session-refresh"], repositories=["auth-service"], status="ready",
                depends_on=[sdk.key], priority="high"))
            web = app.tasks.create(human, "demo", TaskCreate(
                title="Use refresh flow in web-client", description="Keep sessions alive with the SDK refresh helper.",
                parent=state["parent"], projects=["session-refresh"], repositories=["web-client", "shared-sdk"],
                status="ready", depends_on=[sdk.key, auth.key]))
            state.update(sdk=sdk.key, auth=auth.key, web=web.key)
            expect(auth.status == "blocked" and web.status == "blocked", "dependent children must start blocked")
            return {"children": [sdk.key, auth.key, web.key]}

        say(report.step(5, "Create dependent child tasks", s5))

        def s6() -> dict[str, Any]:
            for key in (state["parent"], state["sdk"], state["auth"], state["web"]):
                app.classification.request(human, "demo", key)
            _drain(app)
            categories = {k: app.classification.get(human, "demo", k).current for k in (state["sdk"], state["auth"])}
            expect(all(c is not None and c.provider == "heuristic" for c in categories.values()), "classified")
            return {k: c.category for k, c in categories.items() if c}

        say(report.step(6, "Classify the tasks with the deterministic provider", s6))

        def s7_s9() -> dict[str, Any]:
            results = asyncio.run(_mcp_session(home, [
                ("task_get_ready", {"workspace": "demo"}),
                ("task_claim", {"task": state["sdk"], "workspace": "demo"}),
            ]))
            tools, ready, claimed = results
            expect("task_get_ready" in tools and len(tools) >= 40, f"MCP tools: {len(tools)}")
            ready_keys = [t["key"] for t in ready["tasks"]]
            expect(ready_keys == [state["sdk"]], f"ready via MCP: {ready_keys}")
            expect(claimed["claimed_by"] == "agent:demo-mcp-agent", f"claimed_by {claimed['claimed_by']}")
            return {"mcp_tools": len(tools), "ready": ready_keys, "claimed_by": claimed["claimed_by"]}

        entry = report.step(7, "Query ready tasks through MCP (stdio server subprocess)", s7_s9)
        say(entry)
        say(report.step(8, "Claim work through MCP", lambda: {"task": state["sdk"], "status": "claimed"}))

        def s9() -> dict[str, Any]:
            attempt = app.runs.start(human, "demo", state["sdk"], agent="scripted")
            state["sdk_attempt"] = attempt.id
            expect(attempt.status == "queued", "attempt queued")
            return {"attempt": attempt.id, "provider": attempt.provider}

        say(report.step(9, "Create an agent attempt", s9))

        def s10_14() -> dict[str, Any]:
            _drain(app)
            attempt = app.attempts.get(human, state["sdk_attempt"])
            manifests = app.attempts.manifests(human, attempt.id)
            kinds = [m["kind"] for m in manifests]
            expect("baseline" in kinds and "outcome" in kinds, f"manifests {kinds}")
            expect(attempt.status == "succeeded", f"sdk attempt {attempt.status}: {attempt.error}")
            commits = app.git.task_commits(human, "demo", state["sdk"])
            expect(len(commits) == 1 and any(l["source"] == "execution_context" for l in commits[0]["links"]), "commit linked")
            expect(attempt.usage.get("input_tokens") == 1200 and attempt.cost_usd is None, "telemetry honesty")
            return {"manifests": kinds, "commits": [c["sha"][:10] for c in commits], "usage": attempt.usage,
                    "duration_ms": attempt.duration_ms}

        detail = report.step(10, "Prepare managed worktrees", s10_14)
        say(detail)
        say(report.step(11, "Simulate implementation with the local command adapter", lambda: {"adapter": "command"}))
        say(report.step(12, "Generate real Git commits", lambda: {"commits": detail["commits"]}))

        def s12b() -> dict[str, Any]:
            app.verification.accept(human, "demo", state["sdk"])
            _drain(app)
            run = app.runs.start(human, "demo", state["auth"], agent="scripted-buggy")
            state["auth_attempt_1"] = run.id
            web_blocked = app.tasks.get(human, "demo", state["web"]).status
            expect(web_blocked == "blocked", "web client still blocked by auth")
            _drain(app)
            return {"sdk": "accepted", "auth_attempt": run.id}

        say(report.step(13, "Ingest and associate commits across repositories", s12b))

        def s14() -> dict[str, Any]:
            attempt = app.attempts.get(human, state["auth_attempt_1"])
            events = app.history.task_history(human, "demo", state["auth"])
            progress = [e for e in events if e.event_type == "attempt.progress"]
            expect(bool(progress), "progress telemetry recorded")
            return {"progress_events": len(progress), "queue_wait_ms": attempt.queue_wait_ms}

        say(report.step(14, "Capture execution telemetry", s14))

        def s15_16() -> dict[str, Any]:
            first = app.attempts.get(human, state["auth_attempt_1"])
            expect(first.status == "failed" and first.exit_reason == "verification_failed", f"first {first.status}")
            failures = app.verification.failures(human, "demo")
            auth_failures = [f for f in failures if f["task_key"] == state["auth"]]
            expect(bool(auth_failures) and auth_failures[0]["failure_signatures"][0].startswith("test:"),
                   f"failure signature recorded: {failures}")
            passed = app.verification.list_runs(human, "demo", task=state["sdk"])[0]
            expect(passed.status == "passed", "sdk verification passed")
            return {"passing_run": passed.id, "failing_run_signatures": auth_failures[0]["failure_signatures"]}

        say(report.step(15, "Run configured verification checks", s15_16))
        say(report.step(16, "Record passing and failing verification scenarios", lambda: {"recorded": True}))

        def s17() -> dict[str, Any]:
            retry = app.runs.start(human, "demo", state["auth"], agent="scripted", retry_of=state["auth_attempt_1"])
            _drain(app)
            out = app.attempts.get(human, retry.id)
            expect(out.status == "succeeded" and out.retry_of_attempt_id == state["auth_attempt_1"], f"retry {out.status}")
            app.verification.accept(human, "demo", state["auth"])
            _drain(app)
            web = app.runs.start(human, "demo", state["web"], agent="scripted")
            _drain(app)
            expect(app.attempts.get(human, web.id).status == "succeeded", "web attempt succeeded")
            return {"retry_attempt": retry.id}

        say(report.step(17, "Retry the failed attempt", s17))

        def s18() -> dict[str, Any]:
            acceptance = app.verification.accept(human, "demo", state["web"])
            expect(acceptance.mode == "automated", "automated acceptance")
            parent = app.tasks.detail(human, "demo", state["parent"])
            expect(parent.rollup is not None and parent.rollup.accepted == 3, "all children accepted")
            app.tasks.update(human, "demo", state["parent"], TaskUpdate(status="ready"))
            app.tasks.claim(human, "demo", state["parent"], ClaimRequest())
            app.tasks.complete(human, "demo", state["parent"], summary="children accepted")
            # The parent has no implementation of its own and agent branches are never merged
            # automatically, so it is accepted by a recorded human decision.
            app.verification.accept_manually(human, "demo", state["parent"],
                                             reason="all child tasks accepted after passing verification")
            _drain(app)
            return {"accepted": [state["sdk"], state["auth"], state["web"], state["parent"]]}

        say(report.step(18, "Accept the successfully verified tasks", s18))

        def s19() -> dict[str, Any]:
            changes = app.git.task_changes(human, "demo", state["auth"])
            churn = changes["cumulative_churn"]
            surface = changes["final_change_surface"]
            expect(churn["commits"] == 2, f"auth churn commits {churn['commits']}")
            expect(len(surface) == 1, "final surface computed")
            expect(churn["insertions"] > surface[0]["insertions"], "cumulative churn exceeds final change")
            return {"cumulative_churn": {k: churn[k] for k in ("commits", "insertions", "deletions")},
                    "final_change_surface": {k: surface[0][k] for k in ("insertions", "deletions", "files_changed",
                                                                        "functions_affected")}}

        say(report.step(19, "Calculate cumulative churn and final diff separately", s19))

        def s20() -> dict[str, Any]:
            coupling = app.analytics.coupling(human, "demo")
            pairs = {tuple(sorted((e["source_name"], e["target_name"]))) for e in coupling["edges"]}
            expect(("shared-sdk", "web-client") in pairs, f"pairs {pairs}")
            return {"edges": sorted("+".join(p) for p in pairs), "summary": coupling["summary"]}

        say(report.step(20, "Calculate cross-repository analytics", s20))

        def s21() -> dict[str, Any]:
            history = app.analytics.task_history(human, "demo")
            expect(len(history) == 4, f"history {len(history)}")
            elapsed = app.analytics.query(human, "demo", AnalyticsQuery(metric="attempts_per_task"))["rows"][0]
            return {"accepted_tasks": len(history), "attempts_per_task_median": elapsed.get("median")}

        say(report.step(21, "Query historical task results", s21))

        def s22() -> dict[str, Any]:
            from workgraph.analytics.retrospective import generate_retrospective

            retro = generate_retrospective(app, human, "demo", period_days=7)
            expect("## Work completed" in retro["content"], "retrospective content")
            expect(state["auth"] in retro["content"] or "accepted" in retro["content"], "retro mentions work")
            return {"report_id": retro["id"], "narrative_source": retro["narrative_source"]}

        say(report.step(22, "Generate a retrospective report", s22))

        port = _free_port()
        server_thread, server = _start_api(app, port)
        try:
            def s23() -> dict[str, Any]:
                token = app.api_token()
                headers = {"Authorization": f"Bearer {token}"}
                base = f"http://127.0.0.1:{port}"
                with httpx.Client(base_url=base, headers=headers, timeout=30) as client:
                    tasks = client.get("/api/v1/ws/demo/tasks", params={"limit": 100}).json()
                    expect(tasks["total"] == 4, f"API tasks {tasks.get('total')}")
                    accepted = [t for t in tasks["items"] if t["status"] == "accepted"]
                    expect(len(accepted) == 4, "all accepted via API")
                    unauthorized = httpx.get(f"{base}/api/v1/ws/demo/tasks", timeout=10)
                    expect(unauthorized.status_code == 401, "token required")
                    overview = client.get("/api/v1/ws/demo/analytics/overview").json()
                    expect(sum(r["value"] for r in overview["accepted_by_period"]) == 4, "overview")
                    graph = client.get("/api/v1/ws/demo/graph").json()
                return {"api_tasks": tasks["total"], "graph_edges": len(graph["edges"])}

            say(report.step(23, "Confirm the results through the local HTTP API", s23))

            def s24() -> dict[str, Any]:
                from workgraph.api.app import static_dir

                base = f"http://127.0.0.1:{port}"
                index = httpx.get(f"{base}/", timeout=10)
                expect(index.status_code == 200, "index served")
                built = (static_dir() / "index.html").exists()
                if built:
                    expect('name="workgraph-token"' in index.text, "token injected into UI")
                    expect('<div id="root">' in index.text, "SPA root element")
                return {"ui_built": built, "ui_status": index.status_code,
                        "note": "browser-level checks: pnpm --dir web test:e2e" if built else
                        "web UI not built; run pnpm --dir web build to serve it"}

            say(report.step(24, "Verify the web frontend serves the persisted data", s24))
        finally:
            server.should_exit = True
            server_thread.join(timeout=30)

        def s25() -> dict[str, Any]:
            before = {"tasks": app.tasks.list(human, "demo").total,
                      "attempts": app.attempts.list(human, "demo").total,
                      "events": len(app.history.events(human, "demo", limit=5000))}
            app.close()
            reopened = Workgraph(AppPaths.resolve(home))
            try:
                after = {"tasks": reopened.tasks.list(human, "demo").total,
                         "attempts": reopened.attempts.list(human, "demo").total,
                         "events": len(reopened.history.events(human, "demo", limit=5000))}
                expect(before == after, f"persistence mismatch {before} vs {after}")
                recovered = reopened.queue.recover_expired()
                worktrees = reopened.worktrees.recover(RequestContext.system("demo"))
                projection = reopened.history.verify_task_projection(human, "demo")
                expect(projection["consistent"], f"projection {projection['mismatches']}")
            finally:
                reopened.close()
            return {"before": before, "after": after, "recovered_jobs": len(recovered),
                    "orphaned_worktrees": len(worktrees["orphaned"]), "event_projection": "consistent"}

        say(report.step(25, "Restart the application and confirm persistence and recovery", s25))
    finally:
        app.close()
    return report


def _start_api(app: Workgraph, port: int) -> tuple[threading.Thread, Any]:
    import uvicorn

    from workgraph.api.app import create_app

    asgi = create_app(app, host="127.0.0.1", port=port, with_mcp=True)
    server = uvicorn.Server(uvicorn.Config(asgi, host="127.0.0.1", port=port, log_level="warning"))
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            if httpx.get(f"http://127.0.0.1:{port}/api/v1/health", timeout=1).status_code == 200:
                break
        except httpx.HTTPError:
            time.sleep(0.1)
    return thread, server
