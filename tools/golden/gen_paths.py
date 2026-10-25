"""Golden vectors for ``workgraph.config.paths`` (GV-18).

Each case fixes an operating system, an environment and a ``WORKGRAPH_HOME`` value, runs the
Python ``AppPaths.resolve()`` under them, and records every resolved location. The platform
inputs are simulated, so the output is the same on every host:

* platformdirs' class for the simulated OS replaces its dispatch on ``sys.platform``;
* ``os.path`` is ``posixpath`` or ``ntpath``, and ``Path`` in the paths module is a pure path
  flavour whose ``expanduser`` reads ``HOME`` or ``USERPROFILE`` from the simulated environment
  and whose ``resolve`` is lexical (no filesystem access);
* the Windows Known Folder lookup is replaced by platformdirs' own ``WIN_PD_OVERRIDE_LOCAL_APPDATA``
  hook, set to the same value as ``LOCALAPPDATA``.

Writes ``golden/paths/vectors.json``. Run from the repository root with
``uv run python tools/golden/gen_paths.py``.
"""

from __future__ import annotations

import json
import ntpath
import os
import posixpath
from contextlib import ExitStack
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Any
from unittest.mock import patch

import platformdirs
import platformdirs.macos
import platformdirs.unix
import platformdirs.windows

from workgraph.config import paths as paths_module

ROOT = Path(__file__).resolve().parents[2]
OUT_DIR = ROOT / "golden" / "paths"
OUT_FILE = OUT_DIR / "vectors.json"

WINDOWS_LOCAL_APPDATA = "C:\\Users\\Ada\\AppData\\Local"

# Environment every case starts from, per simulated operating system.
BASE_ENV: dict[str, dict[str, str]] = {
    "windows": {
        "USERPROFILE": "C:\\Users\\Ada",
        "LOCALAPPDATA": WINDOWS_LOCAL_APPDATA,
        "WIN_PD_OVERRIDE_LOCAL_APPDATA": WINDOWS_LOCAL_APPDATA,
    },
    "linux": {"HOME": "/home/ada"},
    "macos": {"HOME": "/Users/ada"},
}

PLATFORM_CLASS = {
    "windows": platformdirs.windows.Windows,
    "linux": platformdirs.unix.Unix,
    "macos": platformdirs.macos.MacOS,
}

OS_FLAVOUR = {
    "windows": (PureWindowsPath, ntpath, "USERPROFILE"),
    "linux": (PurePosixPath, posixpath, "HOME"),
    "macos": (PurePosixPath, posixpath, "HOME"),
}

# WORKGRAPH_HOME values, per operating system (None means the variable is unset).
HOME_VARIANTS: dict[str, dict[str, str | None]] = {
    "unset": {"windows": None, "linux": None, "macos": None},
    "empty": {"windows": "", "linux": "", "macos": ""},
    "absolute": {"windows": "D:\\wg-home", "linux": "/srv/wg-home", "macos": "/srv/wg-home"},
    "tilde": {"windows": "~/wg-home", "linux": "~/wg-home", "macos": "~/wg-home"},
    "dotted": {"windows": "D:\\wg-home\\.\\", "linux": "/srv/wg-home/./", "macos": "/srv/wg-home/./"},
}

XDG_VARIANTS: dict[str, dict[str, str]] = {
    "unset": {},
    "absolute": {
        "XDG_CONFIG_HOME": "/xdg/config",
        "XDG_DATA_HOME": "/xdg/data",
        "XDG_STATE_HOME": "/xdg/state",
    },
    "relative": {
        "XDG_CONFIG_HOME": "relative/config",
        "XDG_DATA_HOME": "relative/data",
        "XDG_STATE_HOME": "relative/state",
    },
}

RESOLVED_ATTRIBUTES = (
    "config_dir",
    "data_dir",
    "log_dir",
    "config_file",
    "workspace_config_dir",
    "database_file",
    "analytics_file",
    "artifacts_dir",
    "worktrees_dir",
    "spool_dir",
    "backups_dir",
    "runs_dir",
    "api_token_file",
)


def _sim_path_class(os_name: str) -> type:
    """A pure path flavour whose expanduser and resolve need no host state."""
    base, _, home_var = OS_FLAVOUR[os_name]

    class SimPath(base):  # type: ignore[valid-type, misc]
        def expanduser(self) -> SimPath:
            text = str(self)
            if text == "~" or text.startswith(("~/", "~\\")):
                return type(self)(os.environ[home_var] + text[1:])
            return self

        def resolve(self, strict: bool = False) -> SimPath:
            return self

    return SimPath


def _environment(os_name: str, home: str | None, xdg: dict[str, str]) -> dict[str, str]:
    env = dict(BASE_ENV[os_name])
    if home is not None:
        env["WORKGRAPH_HOME"] = home
    env.update(xdg)
    return dict(sorted(env.items()))


def _resolve(os_name: str, env: dict[str, str]) -> dict[str, str]:
    _, flavour_os, _ = OS_FLAVOUR[os_name]
    with ExitStack() as stack:
        stack.enter_context(patch.dict(os.environ, env, clear=True))
        stack.enter_context(patch("os.path", flavour_os))
        stack.enter_context(patch.object(platformdirs, "PlatformDirs", PLATFORM_CLASS[os_name]))
        stack.enter_context(patch.object(paths_module, "Path", _sim_path_class(os_name)))
        app = paths_module.AppPaths.resolve()
        return {name: str(getattr(app, name)) for name in RESOLVED_ATTRIBUTES}


def _cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    for os_name in ("windows", "linux", "macos"):
        for home_name, homes in HOME_VARIANTS.items():
            for xdg_name, xdg in XDG_VARIANTS.items():
                env = _environment(os_name, homes[os_name], xdg)
                cases.append(
                    {
                        "id": f"{os_name}-home-{home_name}-xdg-{xdg_name}",
                        "os": os_name,
                        "env": env,
                        "expected": _resolve(os_name, env),
                    }
                )
    return cases


def main() -> None:
    document = {
        "platformdirs": platformdirs.__version__,
        "module": "workgraph.config.paths",
        "cases": _cases(),
    }
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    text = json.dumps(document, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    with OUT_FILE.open("w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
    print(f"wrote {len(document['cases'])} cases to {OUT_FILE}")


if __name__ == "__main__":
    main()
