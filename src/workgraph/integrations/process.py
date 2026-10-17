"""Safe external process execution with timeouts, cancellation and output capture.

* Argument vectors only (``shell=False``).
* The child runs in its own process group / session so a timeout or cancellation
  terminates the whole process tree, on Windows as well as POSIX.
* Output is streamed line by line to an optional callback and to a log file, and
  bounded in memory.
"""

from __future__ import annotations

import contextlib
import os
import shutil
import signal
import subprocess
import sys
import threading
import time
from collections import deque
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import IO

from workgraph.core.errors import ProviderUnavailableError, ValidationFailed

_SAFE_ENV_KEYS = {
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "TEMP",
    "TMP",
    "TMPDIR",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "LANG",
    "LC_ALL",
    "TERM",
    "USER",
    "USERNAME",
    "LOGNAME",
    "SHELL",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "VIRTUAL_ENV",
    "UV_CACHE_DIR",
    "PYTHONUTF8",
    "HOMEDRIVE",
    "HOMEPATH",
}


def base_environment(extra: Mapping[str, str] | None = None, *, inherit_all: bool = False) -> dict[str, str]:
    """A minimal environment for child processes (secrets are not inherited by default)."""
    if inherit_all:
        env = dict(os.environ)
    else:
        env = {k: v for k, v in os.environ.items() if k.upper() in _SAFE_ENV_KEYS}
    env.update(extra or {})
    return env


def resolve_executable(name: str) -> str:
    if "\x00" in name or not name.strip():
        raise ValidationFailed("invalid executable name")
    path = Path(name)
    if path.is_absolute() or len(path.parts) > 1:
        if path.exists():
            return str(path)
        raise ProviderUnavailableError(f"executable {name} does not exist")
    found = shutil.which(name)
    if found is None:
        raise ProviderUnavailableError(f"executable {name!r} was not found on PATH")
    return found


@dataclass
class ProcessResult:
    argv: list[str]
    exit_code: int | None
    duration_ms: int
    timed_out: bool = False
    cancelled: bool = False
    tail: list[str] = field(default_factory=list)
    lines: int = 0

    @property
    def ok(self) -> bool:
        return self.exit_code == 0 and not self.timed_out and not self.cancelled


def _kill_tree(proc: subprocess.Popen[bytes]) -> None:
    if proc.poll() is not None:
        return
    if sys.platform == "win32":
        subprocess.run(
            ["taskkill", "/F", "/T", "/PID", str(proc.pid)], capture_output=True, check=False, timeout=30
        )
    else:
        try:
            os.killpg(proc.pid, signal.SIGTERM)
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()


def run_process(
    argv: Sequence[str],
    *,
    cwd: Path | None = None,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    stdin_data: bytes | None = None,
    on_line: Callable[[str], None] | None = None,
    should_cancel: Callable[[], bool] | None = None,
    log_file: Path | None = None,
    tail_lines: int = 200,
    poll_interval: float = 0.25,
) -> ProcessResult:
    if not argv:
        raise ValidationFailed("empty argument vector")
    executable = resolve_executable(argv[0])
    full_argv = [executable, *argv[1:]]
    started = time.monotonic()
    try:
        if sys.platform == "win32":
            proc = subprocess.Popen(
                full_argv,
                cwd=str(cwd) if cwd else None,
                env=dict(env) if env is not None else None,
                stdin=subprocess.PIPE if stdin_data is not None else subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                creationflags=subprocess.CREATE_NEW_PROCESS_GROUP,
            )
        else:
            proc = subprocess.Popen(
                full_argv,
                cwd=str(cwd) if cwd else None,
                env=dict(env) if env is not None else None,
                stdin=subprocess.PIPE if stdin_data is not None else subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
    except OSError as exc:
        raise ProviderUnavailableError(f"failed to start {argv[0]}: {exc}") from exc

    tail: deque[str] = deque(maxlen=tail_lines)
    counter = {"lines": 0}
    log_handle: IO[str] | None = None
    if log_file is not None:
        log_file.parent.mkdir(parents=True, exist_ok=True)
        log_handle = log_file.open("a", encoding="utf-8")

    def reader() -> None:
        assert proc.stdout is not None
        for raw in iter(proc.stdout.readline, b""):
            line = raw.decode("utf-8", errors="replace").rstrip("\r\n")
            counter["lines"] += 1
            tail.append(line)
            if log_handle is not None:
                log_handle.write(line + "\n")
                log_handle.flush()
            if on_line is not None:
                # A faulty consumer must not kill the reader thread.
                with contextlib.suppress(Exception):
                    on_line(line)
        proc.stdout.close()

    thread = threading.Thread(target=reader, name="process-reader", daemon=True)
    thread.start()
    if stdin_data is not None and proc.stdin is not None:
        try:
            proc.stdin.write(stdin_data)
            proc.stdin.close()
        except (BrokenPipeError, OSError):
            pass

    timed_out = False
    cancelled = False
    while True:
        if proc.poll() is not None:
            break
        elapsed = time.monotonic() - started
        if timeout is not None and elapsed > timeout:
            timed_out = True
            _kill_tree(proc)
            break
        if should_cancel is not None and should_cancel():
            cancelled = True
            _kill_tree(proc)
            break
        time.sleep(poll_interval)
    thread.join(timeout=10)
    if log_handle is not None:
        log_handle.close()
    return ProcessResult(
        argv=full_argv,
        exit_code=proc.returncode,
        duration_ms=int((time.monotonic() - started) * 1000),
        timed_out=timed_out,
        cancelled=cancelled,
        tail=list(tail),
        lines=counter["lines"],
    )
