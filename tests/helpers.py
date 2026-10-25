"""Shared helpers for tests that run agents and workers.

Also hosts the implementation switch used by the interface tests:

* ``WORKGRAPH_IMPL=python`` (default) drives the in-process Python forms (Typer's ``CliRunner``,
  Starlette's ``TestClient``, the Python MCP stdio server).
* ``WORKGRAPH_IMPL=rust`` runs the same tests against the binary named by ``WORKGRAPH_RUST_BIN``:
  the CLI helper spawns it, MCP stdio launches it, and the HTTP helper runs ``serve --no-worker``
  on a free port. Setup done through Python services still writes the shared database.
"""

from __future__ import annotations

import os
import socket
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING

import httpx
from mcp.client.stdio import StdioServerParameters

from workgraph.app import Workgraph
from workgraph.config.settings import CommandAgentConfig
from workgraph.demo import scripted_agent
from workgraph.jobs.worker import Worker

if TYPE_CHECKING:
    from starlette.testclient import TestClient

FAKE_AGENT = str(Path(scripted_agent.__file__))

IMPL_MODES = ("python", "rust")
CLI_TIMEOUT_SECONDS = 120


def impl_mode() -> str:
    """The implementation under test, read from ``WORKGRAPH_IMPL`` at call time (default ``python``)."""
    mode = os.environ.get("WORKGRAPH_IMPL", "").strip() or "python"
    if mode not in IMPL_MODES:
        raise ValueError(f"WORKGRAPH_IMPL must be one of {IMPL_MODES}, not {mode!r}")
    return mode


def rust_binary() -> Path:
    """The Rust ``workgraph`` executable named by ``WORKGRAPH_RUST_BIN``; required under ``rust``."""
    raw = os.environ.get("WORKGRAPH_RUST_BIN", "").strip()
    if not raw:
        raise RuntimeError("WORKGRAPH_RUST_BIN must name the Rust workgraph binary when WORKGRAPH_IMPL=rust")
    path = Path(raw)
    if not path.is_file():
        raise RuntimeError(f"WORKGRAPH_RUST_BIN does not exist: {path}")
    return path


@dataclass(frozen=True)
class CliResult:
    exit_code: int
    # str for run_cli and for run_cli_process with text/encoding; bytes for run_cli_process otherwise.
    stdout: str | bytes
    stderr: str | bytes
    # Python mode: Click's combined output. Rust mode: stdout followed by stderr.
    output: str | bytes


def _merged_env(env: Mapping[str, str | None] | None) -> dict[str, str]:
    """``os.environ`` with ``env`` applied on top; a ``None`` value unsets that variable."""
    merged = dict(os.environ)
    for key, value in (env or {}).items():
        if value is None:
            merged.pop(key, None)
        else:
            merged[key] = value
    return merged


def run_cli(args: Sequence[str], env: Mapping[str, str] | None = None) -> CliResult:
    """Run one CLI invocation with extra environment variables and capture its result.

    Pass ``--json`` in ``args`` where a test wants machine-readable output. In python mode the
    flag is handled the way ``main()`` handles it (stripped from argv, sets ``STATE.json``), since
    ``CliRunner`` does not go through ``main()``.
    """
    if impl_mode() == "rust":
        proc = subprocess.run(
            [str(rust_binary()), *args],
            capture_output=True,
            text=True,
            encoding="utf-8",
            env=_merged_env(env),
            stdin=subprocess.DEVNULL,
            timeout=CLI_TIMEOUT_SECONDS,
            check=False,
        )
        return CliResult(proc.returncode, proc.stdout, proc.stderr, proc.stdout + proc.stderr)

    from typer.testing import CliRunner

    from workgraph.cli.common import STATE
    from workgraph.cli.main import app as cli

    argv = [arg for arg in args if arg != "--json"]
    STATE.json = len(argv) != len(args)
    STATE.home = None
    try:
        result = CliRunner().invoke(cli, argv, env=dict(env or {}), catch_exceptions=False)
    finally:
        STATE.close()
        STATE.json = False
        STATE.home = None
    return CliResult(result.exit_code, result.stdout, result.stderr, result.output)


def run_cli_process(
    args: Sequence[str], env: Mapping[str, str | None] | None = None, **popen_kwargs: object
) -> CliResult:
    """Spawn the selected implementation as a real process and capture its raw output.

    Python mode runs ``python -m workgraph``; Rust mode runs the ``WORKGRAPH_RUST_BIN`` executable.
    ``env`` is applied on top of ``os.environ``; a ``None`` value unsets that variable. Extra keyword
    arguments go to ``subprocess.run``. Without ``text``/``encoding`` the streams are bytes.
    """
    if impl_mode() == "rust":
        command = [str(rust_binary()), *args]
    else:
        command = [sys.executable, "-m", "workgraph", *args]
    popen_kwargs.setdefault("timeout", CLI_TIMEOUT_SECONDS)
    proc = subprocess.run(
        command,
        capture_output=True,
        stdin=subprocess.DEVNULL,
        env=_merged_env(env),
        check=False,
        **popen_kwargs,
    )
    return CliResult(proc.returncode, proc.stdout, proc.stderr, proc.stdout + proc.stderr)


def mcp_stdio_params(
    home: Path | str,
    *,
    agent_name: str = "tester",
    grants: Sequence[str] = (),
    env: Mapping[str, str] | None = None,
) -> StdioServerParameters:
    """Stdio launch parameters for ``workgraph mcp`` against the home directory ``home``."""
    args = ["mcp", "--agent-name", agent_name]
    for grant in grants:
        args += ["--grant", grant]
    merged = {**os.environ, "WORKGRAPH_HOME": str(home), **(env or {})}
    if impl_mode() == "rust":
        return StdioServerParameters(command=str(rust_binary()), args=args, env=merged)
    return StdioServerParameters(command=sys.executable, args=["-m", "workgraph", *args], env=merged)


@contextmanager
def api_client(app: Workgraph) -> Iterator[httpx.Client | TestClient]:
    """An HTTP client for the API of ``app``'s home directory.

    Python mode yields a Starlette ``TestClient`` (base ``http://testserver``). Rust mode starts
    ``workgraph serve --no-worker --local`` on a free loopback port and yields an ``httpx.Client``
    for it. Both forms take relative paths. Callers add the bearer token from ``app.api_token()``.
    """
    # Create the token file before any server reads it, so both implementations share one token.
    app.api_token()
    if impl_mode() == "rust":
        with _rust_server(app) as base_url, httpx.Client(base_url=base_url, timeout=60) as client:
            yield client
        return

    from starlette.testclient import TestClient

    from workgraph.api.app import create_app

    previous = os.environ.get("WORKGRAPH_ALLOWED_HOSTS")
    os.environ["WORKGRAPH_ALLOWED_HOSTS"] = "testserver"
    try:
        asgi = create_app(app, host="127.0.0.1", port=8765, with_mcp=True)
        with TestClient(asgi, base_url="http://testserver") as client:
            yield client
    finally:
        if previous is None:
            os.environ.pop("WORKGRAPH_ALLOWED_HOSTS", None)
        else:
            os.environ["WORKGRAPH_ALLOWED_HOSTS"] = previous


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


@contextmanager
def _rust_server(app: Workgraph) -> Iterator[str]:
    port = _free_port()
    base_url = f"http://127.0.0.1:{port}"
    env = {**os.environ, "WORKGRAPH_HOME": str(app.paths.config_dir.parent)}
    command = [
        str(rust_binary()),
        "serve",
        "--no-worker",
        "--local",
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
    ]
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen(
            command, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT
        )
        try:
            deadline = time.monotonic() + 30
            while True:
                if proc.poll() is not None:
                    log.seek(0)
                    text = log.read().decode(errors="replace")
                    raise RuntimeError(f"workgraph serve exited early: {text}")
                try:
                    if httpx.get(f"{base_url}/api/v1/health", timeout=1).status_code == 200:
                        break
                except httpx.HTTPError:
                    pass
                if time.monotonic() > deadline:
                    raise RuntimeError(f"workgraph serve did not become healthy on port {port}")
                time.sleep(0.1)
            yield base_url
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


def enable_fake_agent(
    app: Workgraph, name: str = "fake", mode: str = "success", *, timeout: float = 120
) -> None:
    config = app.config
    config.execution.enabled = True
    config.execution.default_agent = name
    config.agents[name] = CommandAgentConfig(
        executable=sys.executable,
        args=[FAKE_AGENT, "--mode", mode, "{manifest}"],
        timeout_seconds=timeout,
        supports_usage=True,
    )
    app.config_store.save_global(config)


def drain(app: Workgraph, timeout: float = 120) -> Worker:
    worker = Worker(app, concurrency=2, poll_interval=0.05, maintenance_interval=0.2)
    worker.run_until_idle(timeout=timeout)
    return worker


def passing_check(name: str = "unit tests") -> dict[str, object]:
    return {"name": name, "kind": "test", "command": [sys.executable, "-c", "print('1 passed')"]}


def failing_check(name: str = "unit tests") -> dict[str, object]:
    code = "print('FAILED tests/test_api.py::test_login - AssertionError'); raise SystemExit(1)"
    return {"name": name, "kind": "test", "command": [sys.executable, "-c", code]}
