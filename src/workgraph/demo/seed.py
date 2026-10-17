"""Demo data for exploring the web UI (invoked explicitly with ``workgraph demo seed``).

Everything is produced through the real services: real Git repositories and commits,
the scripted command agent, real verification commands, the heuristic classifier and
real analytics. Nothing in the UI is hard-coded sample data.
"""

from __future__ import annotations

import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any

from workgraph.app import Workgraph
from workgraph.config.settings import CommandAgentConfig, ModelSelection
from workgraph.core.context import RequestContext
from workgraph.core.models.schemas import (
    CommentCreate,
    ProjectCreate,
    RelationCreate,
    RepositoryRegister,
    TaskCreate,
    TaskUpdate,
    WorkspaceCreate,
)
from workgraph.demo import scripted_agent
from workgraph.demo.scenario import INTEGRATION_SCRIPT, REPOS, _drain, create_repositories

TASKS: list[dict[str, Any]] = [
    {"title": "Fix crash when the login token expires", "repos": ["auth-service"], "agent": "scripted",
     "description": "Users see an exception and the app crashes after one hour.", "priority": "critical"},
    {"title": "Add dark mode to the web client", "repos": ["web-client"], "agent": "scripted",
     "description": "Implement a theme toggle in the UI.", "priority": "medium"},
    {"title": "Refactor SDK request handling", "repos": ["shared-sdk"], "agent": "scripted-buggy", "retry": True,
     "description": "Simplify and restructure the client module.", "priority": "medium"},
    {"title": "Improve token validation performance", "repos": ["auth-service", "shared-sdk"], "agent": "scripted",
     "description": "Token validation is slow; reduce latency with caching.", "priority": "high"},
    {"title": "Document the session refresh flow", "repos": ["web-client"], "agent": "scripted",
     "description": "Write docs and a README section about refresh tokens.", "priority": "low"},
    {"title": "Upgrade SDK dependencies", "repos": ["shared-sdk"], "agent": "scripted-fail",
     "description": "Bump dependency versions and fix deprecations.", "priority": "low"},
    {"title": "Investigate flaky integration tests", "repos": ["auth-service", "web-client"], "agent": "scripted",
     "description": "Research why integration tests fail intermittently.", "priority": "medium"},
]


def seed_demo(app: Workgraph, workspace: str, *, log: Callable[[str], None] = print) -> dict[str, Any]:  # noqa: T201
    human = RequestContext.human("demo")
    config = app.config
    config.llm.classification = ModelSelection(provider="heuristic", model="heuristic-v1")
    config.llm.embeddings = ModelSelection(provider="heuristic", model="hash-embed-256")
    config.llm.summarization = ModelSelection(provider="heuristic", model="heuristic-v1")
    config.execution.enabled = True
    agent = str(Path(scripted_agent.__file__))
    for name, mode in (("scripted", "success"), ("scripted-buggy", "buggy"), ("scripted-fail", "fail")):
        config.agents[name] = CommandAgentConfig(executable=sys.executable, args=[agent, "--mode", mode, "{manifest}"],
                                                 supports_usage=True)
    config.execution.default_agent = "scripted"
    app.config_store.save_global(config)
    log("configured heuristic inference and scripted demo agents")
    app.workspaces.create(human, WorkspaceCreate(slug=workspace, name=workspace.title(), key_prefix="DEMO"),
                          make_default=True)
    paths = create_repositories(app.paths.data_dir / "demo-repos" / workspace)
    for path in paths.values():
        app.repositories.register(human, workspace, RepositoryRegister(path=str(path)))
    app.repositories.add_dependency(human, workspace, "web-client", "shared-sdk", "web client consumes the SDK")
    app.projects.create(human, workspace, ProjectCreate(slug="platform", name="Platform", repositories=list(REPOS)))
    app.projects.create(human, workspace, ProjectCreate(slug="frontend", name="Frontend",
                                                        repositories=["web-client", "shared-sdk"]))
    for name in REPOS:
        app.verification.create_config(human, workspace, scope_type="repository", scope=name, name="contract",
                                       checks=[{"name": f"{name} contract tests", "kind": "test",
                                                "command": [sys.executable, "check.py"]}])
    app.verification.create_config(human, workspace, scope_type="project", scope="platform", name="integration",
                                   checks=[{"name": "cross-repo integration", "kind": "integration",
                                            "command": [sys.executable, "-c", INTEGRATION_SCRIPT], "required": False}])
    app.automation.create_rule(human, workspace, {
        "name": "triage critical bugs",
        "trigger": {"type": "event", "event_types": ["task.created"]},
        "conditions": {"field": "task.priority", "op": "eq", "value": "critical"},
        "actions": [{"type": "add_tags", "tags": ["triage"]},
                    {"type": "notify", "level": "warning", "title": "Critical task {{task.key}}: {{task.title}}"}],
    })
    log("created repositories, projects, verification and automation")
    keys: list[str] = []
    for spec in TASKS:
        project = "frontend" if spec["repos"] == ["web-client"] else "platform"
        task = app.tasks.create(human, workspace, TaskCreate(
            title=spec["title"], description=spec["description"], repositories=spec["repos"], projects=[project],
            priority=spec["priority"], status="ready", acceptance_criteria=["Contract tests pass"]))
        keys.append(task.key)
    parent = app.tasks.create(human, workspace, TaskCreate(
        title="Session reliability initiative", description="Umbrella for session-related improvements.",
        projects=["platform"], repositories=list(REPOS), priority="high"))
    for key in keys[:4]:
        app.tasks.update(human, workspace, key, TaskUpdate(parent=parent.key))
    follow = app.tasks.create(human, workspace, TaskCreate(
        title="Handle refresh failures gracefully", description="Show a re-login prompt when refresh fails.",
        repositories=["web-client"], projects=["frontend"], status="ready", depends_on=[keys[0], keys[1]]))
    app.tasks.add_relation(human, workspace, follow.key, RelationCreate(target=keys[0], relation_type="follows_up"))
    app.tasks.add_comment(human, workspace, keys[0], CommentCreate(body="Reproduced locally: token expiry at 60 min."))
    _drain(app)
    log("created tasks and classified them")
    for spec, key in zip(TASKS, keys, strict=True):
        attempt = app.runs.start(human, workspace, key, agent=spec["agent"])
        _drain(app)
        if spec.get("retry"):
            app.runs.start(human, workspace, key, agent="scripted", retry_of=attempt.id)
            _drain(app)
        status = app.tasks.get(human, workspace, key).status
        if status == "awaiting_review" and key not in keys[5:]:
            app.verification.accept(human, workspace, key)
        log(f"{key}: {app.tasks.get(human, workspace, key).status}")
    _drain(app)
    for key in keys:
        try:
            app.search.embed_task(app.tasks.get(human, workspace, key).id)
        except Exception as exc:  # embeddings are optional enrichment
            log(f"embedding skipped for {key}: {exc}")
    from workgraph.analytics.retrospective import generate_retrospective

    generate_retrospective(app, human, workspace, period_days=14)
    commits = sum(len(app.git.task_commits(human, workspace, k)) for k in keys)
    return {"workspace": workspace, "tasks": app.tasks.list(human, workspace).total,
            "attempts": app.attempts.list(human, workspace).total, "commits": commits,
            "repositories": str(app.paths.data_dir / "demo-repos" / workspace)}
