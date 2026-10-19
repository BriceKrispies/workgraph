"""Mirror tracker issues into Workgraph tasks. Read-only towards the tracker.

Fetching happens outside any write transaction; each page of issues is then applied in a
short transaction as ``system:tracker-sync``. See docs/design/tracker-sync.md.
"""

from __future__ import annotations

import dataclasses
import hashlib
import json
from collections.abc import Callable, Sequence
from datetime import UTC, datetime, timedelta
from typing import TYPE_CHECKING, Any

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from workgraph.config.settings import TrackerConfig
from workgraph.core.clock import ensure_utc, utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import TaskPriority
from workgraph.core.models.schemas import TaskCreate, TaskUpdate
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.core.services.tasks import TaskService, depends_on_reachable, is_ancestor
from workgraph.integrations.trackers.types import (
    SYNC_ACTOR,
    SYNC_COMPONENT,
    PullRequestRef,
    SyncReport,
    TrackerClient,
    TrackerSnapshot,
)
from workgraph.persistence.orm import (
    ExternalItem,
    Project,
    Repository,
    RepositoryRemote,
    SystemCursor,
    Task,
    TaskDependency,
    TaskLink,
    TaskRepository,
)

if TYPE_CHECKING:
    from workgraph.app import Workgraph

ClientFactory = Callable[[TrackerConfig], TrackerClient]
Mapper = Callable[[dict[str, Any], list[dict[str, Any]], TrackerConfig], TrackerSnapshot]

PAGE_SIZE = 50
PULL_REQUEST_KIND = "pull_request"


def default_client_factory(config: TrackerConfig) -> TrackerClient:
    from workgraph.integrations.trackers.jira import make_jira_client

    return make_jira_client(config)


def default_mapper(
    issue: dict[str, Any], dev_pull_requests: list[dict[str, Any]], config: TrackerConfig
) -> TrackerSnapshot:
    from workgraph.integrations.trackers.mapper import map_issue

    return map_issue(issue, dev_pull_requests, config)


class TrackerSyncService(Service):
    def __init__(
        self,
        app: Workgraph,
        *,
        client_factory: ClientFactory | None = None,
        mapper: Mapper | None = None,
    ) -> None:
        super().__init__(app)
        self.client_factory = client_factory or default_client_factory
        self.mapper = mapper or default_mapper

    def connection(self, name: str) -> TrackerConfig:
        found = self.app.config.trackers.get(name)
        if found is None:
            raise NotFoundError(
                f"tracker connection {name!r} is not configured", details={"connection": name}
            )
        return found

    def sync(self, ctx: RequestContext, connection: str, *, full: bool = False) -> SyncReport:
        """Fetch issues changed since the connection's cursor (every issue when ``full``) and apply them."""
        config = self.connection(connection)
        ws_id = self._workspace_id(config)
        report = SyncReport(connection=connection, full=full)
        ctx_sync = dataclasses.replace(
            RequestContext.system(SYNC_COMPONENT), correlation_id=ctx.correlation_id
        )
        since: datetime | None = None
        if not full:
            with self.db.session() as session:
                value = _read_cursor(session, _cursor_name(connection, "updated"))
            if value is not None:
                since = _from_ms(value) - timedelta(seconds=config.overlap_seconds)
        client = self.client_factory(config)
        try:
            page: list[dict[str, Any]] = []
            for issue in client.search_updated(config.projects, since):
                page.append(issue)
                if len(page) == PAGE_SIZE:
                    self._apply_page(client, connection, config, ws_id, ctx_sync, page, report)
                    page = []
            if page:
                self._apply_page(client, connection, config, ws_id, ctx_sync, page, report)
            if full:
                self._reconcile(client, connection, config, ctx_sync, report)
            self._finish(connection, full, report)
        finally:
            client.close()
        return report

    def status(self, ctx: RequestContext, connection: str) -> dict[str, Any]:
        """Cursor, last sync times, tracked task counts and unresolved pull request repositories."""
        config = self.connection(connection)
        with self.db.session() as session:
            cursor = _read_cursor(session, _cursor_name(connection, "updated"))
            last_sync = _read_cursor(session, _cursor_name(connection, "last_sync"))
            last_full = _read_cursor(session, _cursor_name(connection, "last_full_sync"))
            tracked = session.scalar(
                select(func.count()).select_from(ExternalItem).where(ExternalItem.connection == connection)
            )
            missing = session.scalar(
                select(func.count())
                .select_from(ExternalItem)
                .where(ExternalItem.connection == connection, ExternalItem.missing_since.is_not(None))
            )
            unresolved = session.scalars(
                select(TaskLink.repo_full_name)
                .join(ExternalItem, ExternalItem.task_id == TaskLink.task_id)
                .where(
                    ExternalItem.connection == connection,
                    TaskLink.kind == PULL_REQUEST_KIND,
                    TaskLink.repository_id.is_(None),
                    TaskLink.repo_full_name != "",
                )
                .distinct()
                .order_by(TaskLink.repo_full_name)
            ).all()
        return {
            "connection": connection,
            "workspace": config.workspace,
            "projects": list(config.projects),
            "cursor": _from_ms(cursor).isoformat() if cursor is not None else None,
            "last_sync": _from_ms(last_sync).isoformat() if last_sync is not None else None,
            "last_full_sync": _from_ms(last_full).isoformat() if last_full is not None else None,
            "tracked_tasks": tracked or 0,
            "missing": missing or 0,
            "unresolved_pull_request_repos": list(unresolved),
        }

    # ------------------------------------------------------------------ fetching (no transaction open)

    def _workspace_id(self, config: TrackerConfig) -> str:
        with self.db.session() as session:
            return resolve.workspace(session, config.workspace).id

    def _map(
        self, client: TrackerClient, config: TrackerConfig, issue: dict[str, Any], report: SyncReport
    ) -> TrackerSnapshot:
        prs: list[dict[str, Any]] = []
        if config.dev_panel:
            try:
                prs = list(client.dev_pull_requests(issue["id"]))
            except Exception as exc:
                report.errors.append(f"dev panel {issue.get('key', issue['id'])}: {exc}")
        return self.mapper(issue, prs, config)

    def _fetch_referenced(
        self,
        client: TrackerClient,
        config: TrackerConfig,
        ws_id: str,
        searched: list[TrackerSnapshot],
        report: SyncReport,
    ) -> list[TrackerSnapshot]:
        """Parents and blockers outside the page that are not synced yet and live in a configured project."""
        have = {snap.key for snap in searched}
        pending = [key for snap in searched for key in _referenced_keys(snap)]
        fetched: list[TrackerSnapshot] = []
        while pending:
            wanted = [
                key
                for key in dict.fromkeys(pending)
                if key not in have and _project_of(key) in config.projects
            ]
            pending = []
            if not wanted:
                break
            with self.db.session() as session:
                known = set(
                    session.scalars(
                        select(Task.key).where(Task.workspace_id == ws_id, Task.key.in_(wanted))
                    ).all()
                )
            for key in wanted:
                if key in known:
                    continue
                have.add(key)
                try:
                    issue = client.get_issue(key)
                except Exception as exc:
                    report.errors.append(f"fetch {key}: {exc}")
                    continue
                if issue is None:
                    continue
                snap = self._map(client, config, issue, report)
                fetched.append(snap)
                pending.extend(_referenced_keys(snap))
        return fetched

    # ------------------------------------------------------------------ writing

    def _apply_page(
        self,
        client: TrackerClient,
        connection: str,
        config: TrackerConfig,
        ws_id: str,
        ctx: RequestContext,
        issues: list[dict[str, Any]],
        report: SyncReport,
    ) -> None:
        report.fetched += len(issues)
        searched = [self._map(client, config, issue, report) for issue in issues]
        batch = _ordered([*searched, *self._fetch_referenced(client, config, ws_id, searched, report)])
        now = utcnow()
        newest = max((ensure_utc(snap.remote_updated_at) for snap in searched), default=None)
        with self.db.session(write=True) as session:
            for snap in batch:
                self._apply(session, ctx, ws_id, connection, snap, report, now)
            if newest is not None:
                _write_cursor(session, _cursor_name(connection, "updated"), _ms(newest))

    def _apply(
        self,
        session: Session,
        ctx: RequestContext,
        ws_id: str,
        connection: str,
        snap: TrackerSnapshot,
        report: SyncReport,
        now: datetime,
    ) -> None:
        # Outside references are resolved first: they take part in change detection, so a
        # repository registered later, or a parent synced later, is picked up on the next pass.
        repo_ids = _resolve_pull_request_repos(session, ws_id, snap, report)
        parent = _task_by_key(session, ws_id, snap.parent_key) if snap.parent_key else None
        blockers = [
            blocker
            for key in dict.fromkeys(snap.blocked_by_keys)
            if (blocker := _task_by_key(session, ws_id, key)) is not None and blocker.key != snap.key
        ]
        links = {
            "repositories": _repo_list(repo_ids),
            "parent": parent.id if parent is not None else None,
            "blockers": sorted(blocker.id for blocker in blockers),
        }
        digest = _snapshot_hash(snap, links)
        item = session.scalar(
            select(ExternalItem).where(
                ExternalItem.connection == connection, ExternalItem.external_id == snap.external_id
            )
        )
        if item is not None and item.snapshot_hash == digest:
            item.synced_at = now
            task = session.get(Task, item.task_id)
            if task is not None:
                _restore(session, ctx, item, task)
            report.unchanged += 1
            return

        _ensure_project(session, ctx, ws_id, snap.project_key)
        created = item is None
        changed: list[str] = []
        if item is None:
            if _task_by_key(session, ws_id, snap.key) is not None:
                report.errors.append(f"{snap.key}: a local task already uses this key; not mirrored")
                return
            self.app.tasks._create_in_session(
                session,
                ctx,
                ws_id,
                TaskCreate(
                    key=snap.key,
                    title=snap.title,
                    description=snap.description,
                    priority=TaskPriority(snap.priority),
                    assignee=snap.assignee,
                    projects=[snap.project_key.lower()],
                    repositories=_repo_list(repo_ids),
                    parent=parent.key if parent is not None else None,
                ),
            )
            task = _task_by_key(session, ws_id, snap.key)
            if task is None:
                raise RuntimeError(f"task {snap.key} is missing right after its creation")
            item = ExternalItem(
                workspace_id=ws_id, task_id=task.id, connection=connection, external_id=snap.external_id
            )
            session.add(item)
        else:
            task = session.get(Task, item.task_id)
            if task is None:
                raise RuntimeError(f"external item {snap.key} has no task")
            _restore(session, ctx, item, task)
            if snap.key != task.key:
                if not _rename(session, ctx, ws_id, task, snap.key, report):
                    return
                changed.append("key")
            updates = _field_updates(session, task, snap, parent, report)
            if updates:
                changed.extend("parent" if name == "clear_parent" else name for name in updates)
                self.app.tasks._update_in_session(session, ctx, task, TaskUpdate(**updates))

        _store_item(item, connection, snap, digest, now)
        session.flush()
        if _sync_pull_requests(session, task, snap, repo_ids):
            changed.append("pull_requests")
        if _sync_repositories(session, ctx, task, repo_ids):
            changed.append("repositories")
        if _sync_dependencies(session, ctx, self.app.tasks, task, blockers, report):
            changed.append("dependencies")
        if self.app.tasks.apply_tracker_status(session, ctx, task, item) is not None:
            changed.append("status")

        if created:
            report.created += 1
        elif changed:
            report.updated += 1
        else:
            report.unchanged += 1
        if created or changed:
            record_event(
                session,
                ctx,
                event_type="task.synced",
                entity_type="task",
                entity_id=task.id,
                workspace_id=ws_id,
                data={
                    "connection": connection,
                    "external_key": snap.key,
                    "created": created,
                    "changed_fields": sorted(set(changed)),
                },
            )
        session.flush()

    def _reconcile(
        self,
        client: TrackerClient,
        connection: str,
        config: TrackerConfig,
        ctx: RequestContext,
        report: SyncReport,
    ) -> None:
        """Mark issues the full listing no longer returns as missing; archive them after the grace period."""
        listed = {external_id for external_id, _key in client.list_keys(config.projects)}
        now = utcnow()
        grace = timedelta(seconds=config.missing_grace_seconds)
        with self.db.session(write=True) as session:
            missing = 0
            items = session.scalars(select(ExternalItem).where(ExternalItem.connection == connection)).all()
            for item in items:
                task = session.get(Task, item.task_id)
                if item.external_id in listed:
                    if task is not None:
                        _restore(session, ctx, item, task)
                    continue
                since = item.missing_since if item.missing_since is not None else now
                item.missing_since = since
                missing += 1
                if task is not None and task.archived_at is None and now - ensure_utc(since) > grace:
                    _archive(session, ctx, task)
                    report.archived += 1
            report.missing = missing

    def _finish(self, connection: str, full: bool, report: SyncReport) -> None:
        now = _ms(utcnow())
        with self.db.session(write=True) as session:
            _write_cursor(session, _cursor_name(connection, "last_sync"), now)
            if full:
                _write_cursor(session, _cursor_name(connection, "last_full_sync"), now)
            value = _read_cursor(session, _cursor_name(connection, "updated"))
        report.cursor = _from_ms(value) if value is not None else None


# ---------------------------------------------------------------------- module helpers


def _cursor_name(connection: str, suffix: str) -> str:
    return f"tracker:{connection}:{suffix}"


def _ms(value: datetime) -> int:
    return int(ensure_utc(value).timestamp() * 1000)


def _from_ms(value: int) -> datetime:
    return datetime.fromtimestamp(value / 1000, tz=UTC)


def _read_cursor(session: Session, name: str) -> int | None:
    row = session.get(SystemCursor, name)
    return None if row is None else row.value


def _write_cursor(session: Session, name: str, value: int) -> None:
    """Move a named cursor forward; cursors never move backwards."""
    row = session.get(SystemCursor, name)
    if row is None:
        session.add(SystemCursor(name=name, value=value))
    else:
        row.value = max(row.value, value)


def _project_of(key: str) -> str:
    return key.rsplit("-", 1)[0]


def _referenced_keys(snap: TrackerSnapshot) -> list[str]:
    return ([snap.parent_key] if snap.parent_key else []) + list(snap.blocked_by_keys)


def _ordered(snaps: Sequence[TrackerSnapshot]) -> list[TrackerSnapshot]:
    """Parents and blockers come before the issues that reference them; cycles keep their input order."""
    by_key = {snap.key: snap for snap in snaps}
    visited: set[str] = set()
    ordered: list[TrackerSnapshot] = []

    def visit(snap: TrackerSnapshot) -> None:
        if snap.key in visited:
            return
        visited.add(snap.key)
        for ref in _referenced_keys(snap):
            if ref in by_key:
                visit(by_key[ref])
        ordered.append(snap)

    for snap in snaps:
        visit(snap)
    return ordered


def _snapshot_hash(snap: TrackerSnapshot, links: dict[str, Any]) -> str:
    fields = dataclasses.asdict(snap)
    fields.pop("raw", None)
    payload = json.dumps(
        {"snapshot": fields, "links": links}, sort_keys=True, separators=(",", ":"), default=str
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _task_by_key(session: Session, ws_id: str, key: str) -> Task | None:
    return session.scalar(select(Task).where(Task.workspace_id == ws_id, Task.key == key))


def _resolve_repository(session: Session, ws_id: str, repo_normalized_url: str) -> str | None:
    """The registered, non-archived repository of the workspace whose remote matches the URL."""
    return session.scalar(
        select(Repository.id)
        .join(RepositoryRemote, RepositoryRemote.repository_id == Repository.id)
        .where(
            Repository.workspace_id == ws_id,
            Repository.archived_at.is_(None),
            func.lower(RepositoryRemote.normalized_url) == repo_normalized_url.lower(),
        )
        .order_by(Repository.name)
        .limit(1)
    )


def _resolve_pull_request_repos(
    session: Session, ws_id: str, snap: TrackerSnapshot, report: SyncReport
) -> dict[str, str | None]:
    """Repository id per distinct pull request repository (None when not registered)."""
    resolved: dict[str, str | None] = {}
    for pr in snap.pull_requests:
        if pr.repo_normalized_url in resolved:
            continue
        repo_id = _resolve_repository(session, ws_id, pr.repo_normalized_url)
        resolved[pr.repo_normalized_url] = repo_id
        if repo_id is None and pr.repo_normalized_url not in report.unresolved_pull_requests:
            report.unresolved_pull_requests.append(pr.repo_normalized_url)
    return resolved


def _repo_list(repo_ids: dict[str, str | None]) -> list[str]:
    return list(dict.fromkeys(repo_id for repo_id in repo_ids.values() if repo_id is not None))


def _ensure_project(session: Session, ctx: RequestContext, ws_id: str, project_key: str) -> None:
    """The project for a Jira project key, created in the workspace when it does not exist yet."""
    slug = project_key.lower()
    if (
        session.scalar(select(Project.id).where(Project.workspace_id == ws_id, Project.slug == slug))
        is not None
    ):
        return
    project = Project(workspace_id=ws_id, slug=slug, name=project_key)
    session.add(project)
    session.flush()
    record_event(
        session,
        ctx,
        event_type="project.created",
        entity_type="project",
        entity_id=project.id,
        workspace_id=ws_id,
        data={"slug": slug, "name": project_key, "repository_ids": []},
    )


def _store_item(
    item: ExternalItem, connection: str, snap: TrackerSnapshot, digest: str, now: datetime
) -> None:
    item.connection = connection
    item.external_key = snap.key
    item.url = snap.url
    item.tracker_status = snap.status_name
    item.tracker_status_category = snap.status_category
    item.tracker_resolution = snap.resolution
    item.remote_updated_at = snap.remote_updated_at
    item.snapshot = snap.raw
    item.snapshot_hash = digest
    item.synced_at = now
    item.missing_since = None


def _restore(session: Session, ctx: RequestContext, item: ExternalItem, task: Task) -> None:
    """An issue that reappeared: clear its missing mark and unarchive a task only the grace period archived."""
    if item.missing_since is None:
        return
    item.missing_since = None
    if task.archived_at is not None:
        task.archived_at = None
        task.version += 1
        record_event(
            session,
            ctx,
            event_type="task.unarchived",
            entity_type="task",
            entity_id=task.id,
            workspace_id=task.workspace_id,
            data={"key": task.key},
        )


def _archive(session: Session, ctx: RequestContext, task: Task) -> None:
    task.archived_at = utcnow()
    task.version += 1
    record_event(
        session,
        ctx,
        event_type="task.archived",
        entity_type="task",
        entity_id=task.id,
        workspace_id=task.workspace_id,
        data={"key": task.key},
    )


def _rename(
    session: Session, ctx: RequestContext, ws_id: str, task: Task, new_key: str, report: SyncReport
) -> bool:
    if _task_by_key(session, ws_id, new_key) is not None:
        report.errors.append(f"{task.key}: Jira now calls it {new_key}, which a local task already uses")
        return False
    old_key = task.key
    task.key = new_key
    task.version += 1
    record_event(
        session,
        ctx,
        event_type="task.updated",
        entity_type="task",
        entity_id=task.id,
        workspace_id=task.workspace_id,
        data={
            "key": new_key,
            "changes": {"key": [old_key, new_key]},
            "material_change": False,
            "reason": "tracker key change",
            "version": task.version,
        },
    )
    return True


def _field_updates(
    session: Session, task: Task, snap: TrackerSnapshot, parent: Task | None, report: SyncReport
) -> dict[str, Any]:
    """The TaskUpdate fields that differ from the snapshot; only Jira-owned fields are considered."""
    updates: dict[str, Any] = {}
    if snap.title != task.title:
        updates["title"] = snap.title
    if snap.description != task.description:
        updates["description"] = snap.description
    if snap.priority != task.priority:
        updates["priority"] = TaskPriority(snap.priority)
    if snap.assignee != task.assignee:
        updates["assignee"] = snap.assignee
    if parent is None:
        if snap.parent_key is None and task.parent_id is not None:
            updates["clear_parent"] = True
    elif parent.id != task.parent_id:
        if parent.id == task.id or is_ancestor(session, task.id, parent.id):
            report.errors.append(f"{task.key}: parent {parent.key} would create a hierarchy cycle")
        else:
            updates["parent"] = parent.key
    return updates


def _sync_pull_requests(
    session: Session, task: Task, snap: TrackerSnapshot, repo_ids: dict[str, str | None]
) -> bool:
    links = {
        link.normalized_url: link
        for link in session.scalars(
            select(TaskLink).where(TaskLink.task_id == task.id, TaskLink.kind == PULL_REQUEST_KIND)
        ).all()
    }
    wanted: dict[str, PullRequestRef] = {}
    for pr in snap.pull_requests:
        wanted.setdefault(pr.normalized_url, pr)
    changed = False
    for url, pr in wanted.items():
        values: dict[str, Any] = {
            "url": pr.url,
            "provider": pr.provider,
            "repo_full_name": pr.repo_full_name,
            "number": pr.number,
            "repository_id": repo_ids.get(pr.repo_normalized_url),
            "sources": list(pr.sources),
            "title": pr.title,
        }
        link = links.get(url)
        if link is None:
            session.add(
                TaskLink(
                    task_id=task.id,
                    kind=PULL_REQUEST_KIND,
                    normalized_url=url,
                    created_by=SYNC_ACTOR,
                    **values,
                )
            )
            changed = True
            continue
        for attr, value in values.items():
            if getattr(link, attr) != value:
                setattr(link, attr, value)
                changed = True
    for url, link in links.items():
        if url not in wanted and link.created_by == SYNC_ACTOR:
            session.delete(link)
            changed = True
    session.flush()
    return changed


def _sync_repositories(
    session: Session, ctx: RequestContext, task: Task, repo_ids: dict[str, str | None]
) -> bool:
    changed = False
    for repo_id in _repo_list(repo_ids):
        if session.get(TaskRepository, (task.id, repo_id)) is not None:
            continue
        repo = session.get(Repository, repo_id)
        if repo is None:
            continue
        session.add(TaskRepository(task_id=task.id, repository_id=repo_id, access=repo.access_mode))
        task.version += 1
        record_event(
            session,
            ctx,
            event_type="task.repository_linked",
            entity_type="task",
            entity_id=task.id,
            workspace_id=task.workspace_id,
            data={"repository_id": repo_id, "repository": repo.name, "access": repo.access_mode},
        )
        changed = True
    session.flush()
    return changed


def _sync_dependencies(
    session: Session,
    ctx: RequestContext,
    tasks: TaskService,
    task: Task,
    blockers: list[Task],
    report: SyncReport,
) -> bool:
    """Make the sync-created dependencies of ``task`` match its blockers; human-made ones are left alone."""
    existing = {
        dep.depends_on_id: dep
        for dep in session.scalars(select(TaskDependency).where(TaskDependency.task_id == task.id)).all()
    }
    wanted = {blocker.id for blocker in blockers}
    changed = False
    for blocker in blockers:
        if blocker.id in existing:
            continue
        if depends_on_reachable(session, blocker.id, task.id) is not None:
            report.errors.append(f"{task.key}: blocked by {blocker.key} would create a dependency cycle")
            continue
        session.add(TaskDependency(task_id=task.id, depends_on_id=blocker.id, created_by=SYNC_ACTOR))
        _dependency_changed(session, ctx, tasks, task, "task.dependency_added", blocker.id, blocker.key)
        changed = True
    for depends_on_id, dep in existing.items():
        if depends_on_id in wanted or dep.created_by != SYNC_ACTOR:
            continue
        session.delete(dep)
        blocker = session.get(Task, depends_on_id)
        _dependency_changed(
            session,
            ctx,
            tasks,
            task,
            "task.dependency_removed",
            depends_on_id,
            blocker.key if blocker else depends_on_id,
        )
        changed = True
    return changed


def _dependency_changed(
    session: Session,
    ctx: RequestContext,
    tasks: TaskService,
    task: Task,
    event_type: str,
    depends_on_id: str,
    depends_on_key: str,
) -> None:
    session.flush()
    task.version += 1
    event = record_event(
        session,
        ctx,
        event_type=event_type,
        entity_type="task",
        entity_id=task.id,
        workspace_id=task.workspace_id,
        data={"depends_on": depends_on_id, "depends_on_key": depends_on_key},
    )
    tasks._refresh_blocked_state(session, ctx.caused_by(event.id), task)
