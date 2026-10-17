from __future__ import annotations

import os
import subprocess
from collections.abc import Callable, Iterator
from pathlib import Path

import pytest

from workgraph.app import Workgraph
from workgraph.config.paths import AppPaths
from workgraph.core.context import RequestContext
from workgraph.core.models.schemas import WorkspaceCreate

GIT_ENV = {
    "GIT_AUTHOR_NAME": "Test Author",
    "GIT_AUTHOR_EMAIL": "author@example.com",
    "GIT_COMMITTER_NAME": "Test Committer",
    "GIT_COMMITTER_EMAIL": "committer@example.com",
    "GIT_CONFIG_NOSYSTEM": "1",
}


def git(path: Path, *args: str, env: dict[str, str] | None = None) -> str:
    full_env = {**os.environ, **GIT_ENV, **(env or {})}
    full_env["GIT_CONFIG_GLOBAL"] = str(path.parent / ".gitconfig-test")
    proc = subprocess.run(
        ["git", "-C", str(path), *args], capture_output=True, text=True, env=full_env, check=False
    )
    if proc.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {proc.stderr}")
    return proc.stdout.strip()


class GitRepo:
    """A real temporary Git repository for integration tests."""

    def __init__(self, path: Path) -> None:
        self.path = path
        self._tick = 0

    def git(self, *args: str, env: dict[str, str] | None = None) -> str:
        return git(self.path, *args, env=env)

    def write(self, rel: str, content: str) -> Path:
        target = self.path / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8", newline="\n")
        return target

    def commit(self, message: str, files: dict[str, str] | None = None, *, all_changes: bool = True) -> str:
        for rel, content in (files or {}).items():
            self.write(rel, content)
        if all_changes:
            self.git("add", "-A")
        self._tick += 1
        stamp = f"2026-01-01T00:{self._tick // 60:02d}:{self._tick % 60:02d}+00:00"
        self.git(
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            message,
            env={"GIT_AUTHOR_DATE": stamp, "GIT_COMMITTER_DATE": stamp},
        )
        return self.head()

    def head(self) -> str:
        return self.git("rev-parse", "HEAD")


@pytest.fixture
def make_repo(tmp_path: Path) -> Callable[..., GitRepo]:
    def factory(name: str, *, initial: bool = True) -> GitRepo:
        path = tmp_path / "repos" / name
        path.mkdir(parents=True)
        git(path, "init", "-q", "-b", "main")
        git(path, "config", "user.name", "Test Author")
        git(path, "config", "user.email", "author@example.com")
        git(path, "config", "core.autocrlf", "false")
        repo = GitRepo(path)
        if initial:
            repo.commit("initial commit", {"README.md": f"# {name}\n"})
        return repo

    return factory


@pytest.fixture
def home(tmp_path: Path) -> Path:
    return tmp_path / "wg-home"


@pytest.fixture
def app(home: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[Workgraph]:
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
    instance = Workgraph(AppPaths.resolve(home))
    config = instance.config
    # Keep the ordinary suite hermetic: no automatic model jobs unless a test opts in.
    config.llm.auto_classify = False
    config.llm.auto_embed = False
    instance.config_store.save_global(config)
    yield instance
    instance.close()


@pytest.fixture
def human() -> RequestContext:
    return RequestContext.human("tester")


@pytest.fixture
def agent_ctx() -> RequestContext:
    return RequestContext.agent("test-agent")


@pytest.fixture
def ws(app: Workgraph, human: RequestContext) -> str:
    out = app.workspaces.create(
        human, WorkspaceCreate(slug="main", name="Main", key_prefix="WG"), make_default=True
    )
    return out.slug
