"""Shared plumbing for application services."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from workgraph.app import Workgraph


class Service:
    def __init__(self, app: Workgraph) -> None:
        self.app = app
        self.db = app.db
