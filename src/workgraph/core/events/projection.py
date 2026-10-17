"""Deterministic projection of task state from the event log.

Ordinary reads use the current-state tables. This projection exists to audit them:
replaying the log must reproduce the same task state, which the test-suite and
``workgraph doctor --verify-events`` check.
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass, field
from typing import Any

from workgraph.core.events.log import EventRecord


@dataclass
class ProjectedTask:
    id: str
    key: str
    title: str = ""
    description: str = ""
    status: str = "backlog"
    priority: str = "medium"
    tags: list[str] = field(default_factory=list)
    project_ids: list[str] = field(default_factory=list)
    repository_ids: list[str] = field(default_factory=list)
    parent_id: str | None = None
    depends_on: list[str] = field(default_factory=list)
    assignee: str | None = None
    archived: bool = False
    version_events: int = 0

    def comparable(self) -> dict[str, Any]:
        return {
            "key": self.key,
            "title": self.title,
            "description": self.description,
            "status": self.status,
            "priority": self.priority,
            "tags": sorted(self.tags),
            "project_ids": sorted(self.project_ids),
            "repository_ids": sorted(self.repository_ids),
            "parent_id": self.parent_id,
            "depends_on": sorted(self.depends_on),
            "archived": self.archived,
        }


_FIELD_MAP = {
    "title": "title",
    "description": "description",
    "priority": "priority",
    "tags": "tags",
    "project_ids": "project_ids",
    "parent_id": "parent_id",
    "assignee": "assignee",
}


def project_tasks(events: Iterable[EventRecord]) -> dict[str, ProjectedTask]:
    tasks: dict[str, ProjectedTask] = {}
    for event in sorted(events, key=lambda e: e.seq):
        if event.entity_type != "task":
            continue
        data = event.data
        if event.event_type == "task.created":
            tasks[event.entity_id] = ProjectedTask(
                id=event.entity_id,
                key=data["key"],
                title=data.get("title", ""),
                description=data.get("description", ""),
                status=data.get("status", "backlog"),
                priority=data.get("priority", "medium"),
                tags=list(data.get("tags", [])),
                project_ids=list(data.get("project_ids", [])),
                repository_ids=list(data.get("repository_ids", [])),
                parent_id=data.get("parent_id"),
                depends_on=list(data.get("depends_on", [])),
                assignee=data.get("assignee"),
            )
            continue
        task = tasks.get(event.entity_id)
        if task is None:
            continue
        task.version_events += 1
        match event.event_type:
            case "task.updated":
                for name, (_old, new) in data.get("changes", {}).items():
                    attr = _FIELD_MAP.get(name)
                    if attr is not None:
                        setattr(task, attr, list(new) if isinstance(new, list) else new)
            case "task.status_changed":
                task.status = data["to"]
            case "task.dependency_added":
                if data["depends_on"] not in task.depends_on:
                    task.depends_on.append(data["depends_on"])
            case "task.dependency_removed":
                task.depends_on = [d for d in task.depends_on if d != data["depends_on"]]
            case "task.repository_linked":
                if data["repository_id"] not in task.repository_ids:
                    task.repository_ids.append(data["repository_id"])
            case "task.repository_unlinked":
                task.repository_ids = [r for r in task.repository_ids if r != data["repository_id"]]
            case "task.archived":
                task.archived = True
            case "task.unarchived":
                task.archived = False
            case _:
                pass
    return tasks
