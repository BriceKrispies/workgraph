"""Application container wiring storage, configuration and services together.

Every interface (CLI, HTTP API, MCP server, worker) constructs one
:class:`Workgraph` and calls the same service objects, which guarantees identical
validation, authorization and auditing regardless of the entry point.
"""

from __future__ import annotations

import contextlib
import secrets
from functools import cached_property
from pathlib import Path
from typing import TYPE_CHECKING

from workgraph.config.paths import AppPaths
from workgraph.config.settings import ConfigStore, GlobalConfig
from workgraph.core.errors import ConfigError, NotFoundError
from workgraph.core.fsutil import atomic_write_text
from workgraph.persistence import migrate
from workgraph.persistence.db import Database

if TYPE_CHECKING:
    from workgraph.analytics.service import AnalyticsService
    from workgraph.automation.engine import AutomationService
    from workgraph.classification.service import ClassificationService
    from workgraph.core.services.attempts import AttemptService
    from workgraph.core.services.history import HistoryService
    from workgraph.core.services.projects import ProjectService
    from workgraph.core.services.repositories import RepositoryService
    from workgraph.core.services.stacks import StackService
    from workgraph.core.services.tasks import TaskService
    from workgraph.core.services.workspaces import WorkspaceService
    from workgraph.integrations.agents.registry import AgentRegistry
    from workgraph.integrations.claude_code.ingest import HookIngestor
    from workgraph.integrations.git.service import GitService
    from workgraph.integrations.git.stack_integration import StackIntegrator
    from workgraph.integrations.git.stack_landing import StackLander
    from workgraph.integrations.git.stack_verification import StackVerifier
    from workgraph.integrations.git.worktrees import WorktreeManager
    from workgraph.integrations.llm.registry import LLMRegistry
    from workgraph.jobs.queue import JobQueue
    from workgraph.orchestration.runs import RunService
    from workgraph.orchestration.scheduler import Scheduler
    from workgraph.search.service import SearchService
    from workgraph.verification.service import VerificationService


class Workgraph:
    def __init__(self, paths: AppPaths | None = None, *, auto_migrate: bool = True) -> None:
        self.paths = paths or AppPaths.resolve()
        self.paths.ensure()
        self.config_store = ConfigStore(self.paths)
        self.db = Database(self.paths.database_file)
        if auto_migrate and not migrate.is_up_to_date(self.db):
            migrate.upgrade(self.db)

    @classmethod
    def open(cls, home: str | Path | None = None) -> Workgraph:
        return cls(AppPaths.resolve(home))

    def close(self) -> None:
        self.db.dispose()

    # ------------------------------------------------------------------ configuration

    @property
    def config(self) -> GlobalConfig:
        return self.config_store.load_global()

    def effective_config(self, workspace_slug: str | None) -> GlobalConfig:
        return self.config_store.effective(workspace_slug)

    def api_token(self, *, create: bool = True) -> str:
        token_file = self.paths.api_token_file
        if token_file.exists():
            return token_file.read_text(encoding="utf-8").strip()
        if not create:
            raise ConfigError("API token has not been generated; run `workgraph init`")
        token = secrets.token_urlsafe(32)
        atomic_write_text(token_file, token)
        with contextlib.suppress(OSError):  # chmod is best-effort on Windows
            token_file.chmod(0o600)
        return token

    def default_workspace_id(self) -> str:
        from workgraph.core.services import resolve

        slug = self.config.default_workspace
        with self.db.session() as session:
            if slug:
                return resolve.workspace(session, slug).id
            from sqlalchemy import select

            from workgraph.persistence.orm import Workspace

            ids = list(session.scalars(select(Workspace.id).where(Workspace.archived_at.is_(None)).limit(2)))
        if len(ids) == 1:
            return ids[0]
        raise NotFoundError(
            "no default workspace configured; pass --workspace or run `workgraph workspace use <slug>`"
        )

    def workspace_slug(self, workspace_id: str) -> str:
        from workgraph.persistence.orm import Workspace

        with self.db.session() as session:
            ws = session.get(Workspace, workspace_id)
            if ws is None:
                raise NotFoundError(f"workspace {workspace_id} not found")
            return ws.slug

    # ------------------------------------------------------------------ services

    @cached_property
    def queue(self) -> JobQueue:
        from workgraph.jobs.limits import resource_limit_resolver
        from workgraph.jobs.queue import JobQueue

        return JobQueue(self.db, resource_limit_resolver(self))

    @cached_property
    def workspaces(self) -> WorkspaceService:
        from workgraph.core.services.workspaces import WorkspaceService

        return WorkspaceService(self)

    @cached_property
    def projects(self) -> ProjectService:
        from workgraph.core.services.projects import ProjectService

        return ProjectService(self)

    @cached_property
    def repositories(self) -> RepositoryService:
        from workgraph.core.services.repositories import RepositoryService

        return RepositoryService(self)

    @cached_property
    def tasks(self) -> TaskService:
        from workgraph.core.services.tasks import TaskService

        return TaskService(self)

    @cached_property
    def stacks(self) -> StackService:
        from workgraph.core.services.stacks import StackService

        return StackService(self)

    @cached_property
    def history(self) -> HistoryService:
        from workgraph.core.services.history import HistoryService

        return HistoryService(self)

    @cached_property
    def attempts(self) -> AttemptService:
        from workgraph.core.services.attempts import AttemptService

        return AttemptService(self)

    @cached_property
    def git(self) -> GitService:
        from workgraph.integrations.git.service import GitService

        return GitService(self)

    @cached_property
    def worktrees(self) -> WorktreeManager:
        from workgraph.integrations.git.worktrees import WorktreeManager

        return WorktreeManager(self)

    @cached_property
    def stack_integration(self) -> StackIntegrator:
        from workgraph.integrations.git.stack_integration import StackIntegrator

        return StackIntegrator(self)

    @cached_property
    def stack_verification(self) -> StackVerifier:
        from workgraph.integrations.git.stack_verification import StackVerifier

        return StackVerifier(self)

    @cached_property
    def stack_lander(self) -> StackLander:
        from workgraph.integrations.git.stack_landing import StackLander

        return StackLander(self)

    @cached_property
    def llm(self) -> LLMRegistry:
        from workgraph.integrations.llm.registry import LLMRegistry

        return LLMRegistry(self)

    @cached_property
    def classification(self) -> ClassificationService:
        from workgraph.classification.service import ClassificationService

        return ClassificationService(self)

    @cached_property
    def search(self) -> SearchService:
        from workgraph.search.service import SearchService

        return SearchService(self)

    @cached_property
    def agents(self) -> AgentRegistry:
        from workgraph.integrations.agents.registry import AgentRegistry

        return AgentRegistry(self)

    @cached_property
    def runs(self) -> RunService:
        from workgraph.orchestration.runs import RunService

        return RunService(self)

    @cached_property
    def scheduler(self) -> Scheduler:
        from workgraph.orchestration.scheduler import Scheduler

        return Scheduler(self)

    @cached_property
    def verification(self) -> VerificationService:
        from workgraph.verification.service import VerificationService

        return VerificationService(self)

    @cached_property
    def automation(self) -> AutomationService:
        from workgraph.automation.engine import AutomationService

        return AutomationService(self)

    @cached_property
    def analytics(self) -> AnalyticsService:
        from workgraph.analytics.service import AnalyticsService

        return AnalyticsService(self)

    @cached_property
    def hooks(self) -> HookIngestor:
        from workgraph.integrations.claude_code.ingest import HookIngestor

        return HookIngestor(self)
