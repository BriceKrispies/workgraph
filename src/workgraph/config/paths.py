"""Platform-appropriate storage locations.

The default layout follows :mod:`platformdirs`. Setting ``WORKGRAPH_HOME`` relocates
everything under a single directory (used by tests, demos and portable installs).
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

import platformdirs

APP_NAME = "workgraph"


@dataclass(frozen=True)
class AppPaths:
    config_dir: Path
    data_dir: Path
    log_dir: Path

    @classmethod
    def resolve(cls, home: Path | str | None = None) -> AppPaths:
        root = home if home is not None else os.environ.get("WORKGRAPH_HOME")
        if root:
            base = Path(root).expanduser().resolve()
            return cls(config_dir=base / "config", data_dir=base / "data", log_dir=base / "logs")
        return cls(
            config_dir=Path(platformdirs.user_config_dir(APP_NAME, appauthor=False)),
            data_dir=Path(platformdirs.user_data_dir(APP_NAME, appauthor=False)),
            log_dir=Path(platformdirs.user_log_dir(APP_NAME, appauthor=False)),
        )

    @property
    def config_file(self) -> Path:
        return self.config_dir / "config.toml"

    @property
    def workspace_config_dir(self) -> Path:
        return self.config_dir / "workspaces"

    @property
    def database_file(self) -> Path:
        return self.data_dir / "workgraph.db"

    @property
    def analytics_file(self) -> Path:
        return self.data_dir / "analytics.duckdb"

    @property
    def artifacts_dir(self) -> Path:
        return self.data_dir / "artifacts"

    @property
    def worktrees_dir(self) -> Path:
        return self.data_dir / "worktrees"

    @property
    def spool_dir(self) -> Path:
        return self.data_dir / "spool" / "claude-hooks"

    @property
    def backups_dir(self) -> Path:
        return self.data_dir / "backups"

    @property
    def runs_dir(self) -> Path:
        return self.data_dir / "runs"

    @property
    def api_token_file(self) -> Path:
        return self.config_dir / "api-token"

    def ensure(self) -> None:
        for directory in (
            self.config_dir,
            self.workspace_config_dir,
            self.data_dir,
            self.log_dir,
            self.artifacts_dir,
            self.worktrees_dir,
            self.spool_dir,
            self.backups_dir,
            self.runs_dir,
        ):
            directory.mkdir(parents=True, exist_ok=True)
