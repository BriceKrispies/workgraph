"""Headless screenshots of the web UI through an installed Chromium-family browser.

No browser is bundled: Chrome, Edge or Chromium is located on the machine and driven over the
Chrome DevTools Protocol. Loading the page with ``--screenshot`` alone is not enough, because the UI
fetches its data after first paint and keeps an event stream open, so the capture waits for a CSS
selector to appear and for the layout to settle before taking the picture.
"""

from __future__ import annotations

import base64
import contextlib
import itertools
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Any

from workgraph.core.errors import ProviderUnavailableError, ValidationFailed

if TYPE_CHECKING:
    from websockets.sync.client import ClientConnection

BROWSER_ENV = "WORKGRAPH_BROWSER"

_NAMES = ("chrome", "google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "msedge")


def _known_paths() -> list[Path]:
    if sys.platform == "win32":
        roots = [os.environ.get(k) for k in ("PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA")]
        rel = (
            "Google/Chrome/Application/chrome.exe",
            "Microsoft/Edge/Application/msedge.exe",
            "Chromium/Application/chrome.exe",
        )
        return [Path(r) / p for r in roots if r for p in rel]
    if sys.platform == "darwin":
        return [
            Path("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            Path("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            Path("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        ]
    return []


def find_browser(explicit: str | None = None) -> Path:
    """The browser to drive: ``explicit``, then ``$WORKGRAPH_BROWSER``, then PATH and install locations."""
    chosen = explicit or os.environ.get(BROWSER_ENV)
    if chosen:
        path = Path(chosen)
        if path.is_file():
            return path
        found = shutil.which(chosen)
        if found:
            return Path(found)
        raise ProviderUnavailableError(f"browser not found: {chosen}")
    for name in _NAMES:
        found = shutil.which(name)
        if found:
            return Path(found)
    for path in _known_paths():
        if path.is_file():
            return path
    raise ProviderUnavailableError(
        f"no Chrome, Edge or Chromium found; install one or set {BROWSER_ENV} to its executable"
    )


@dataclass(frozen=True)
class Capture:
    url: str
    path: Path
    width: int
    height: int
    scale: float
    browser: Path


class _Cdp:
    """Minimal synchronous DevTools client for one page target."""

    def __init__(self, ws: ClientConnection) -> None:
        self._ws = ws
        self._ids = itertools.count(1)

    def call(self, method: str, timeout: float = 30, **params: Any) -> dict[str, Any]:
        msg_id = next(self._ids)
        self._ws.send(json.dumps({"id": msg_id, "method": method, "params": params}))
        deadline = time.monotonic() + timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise ProviderUnavailableError(f"browser did not answer {method} within {timeout:.0f}s")
            reply = json.loads(self._ws.recv(timeout=remaining))
            if reply.get("id") != msg_id:
                continue  # an event, or a reply to something else
            if "error" in reply:
                raise ProviderUnavailableError(f"browser refused {method}: {reply['error'].get('message')}")
            return reply.get("result", {})

    def evaluate(self, expression: str) -> Any:
        result = self.call("Runtime.evaluate", expression=expression, returnByValue=True)
        return result.get("result", {}).get("value")


def _devtools_port(profile: Path, proc: subprocess.Popen[bytes], timeout: float) -> int:
    marker = profile / "DevToolsActivePort"
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise ProviderUnavailableError(f"browser exited during start-up (code {proc.returncode})")
        with contextlib.suppress(OSError, ValueError, IndexError):
            return int(marker.read_text().splitlines()[0])
        time.sleep(0.1)
    raise ProviderUnavailableError("browser did not open a DevTools port")


def _page_websocket(port: int, timeout: float) -> str:
    import httpx

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        with contextlib.suppress(httpx.HTTPError):
            targets = httpx.get(f"http://127.0.0.1:{port}/json/list", timeout=5).json()
            pages = [t for t in targets if t.get("type") == "page" and t.get("webSocketDebuggerUrl")]
            if pages:
                return str(pages[0]["webSocketDebuggerUrl"])
        time.sleep(0.1)
    raise ProviderUnavailableError("browser opened no page target")


def screenshot(
    url: str,
    out: Path,
    *,
    width: int = 1600,
    height: int = 1000,
    scale: float = 1.0,
    wait_for: str | None = None,
    wait_gone: str | None = '[aria-busy="true"]',
    settle: float = 1.0,
    timeout: float = 60.0,
    browser: str | None = None,
) -> Capture:
    """Load ``url`` headless and write a PNG of the viewport to ``out``.

    Waits for the page to finish loading, for ``wait_for`` (a CSS selector) to match and
    ``wait_gone`` to stop matching (by default: nothing marked ``aria-busy``), then ``settle``
    seconds for layout and animations.
    """
    if width < 200 or height < 200 or not 0.25 <= scale <= 4:
        raise ValidationFailed("width and height must be at least 200 and scale between 0.25 and 4")
    exe = find_browser(browser)
    out = out.resolve()
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="workgraph-browser-", ignore_cleanup_errors=True) as profile_dir:
        profile = Path(profile_dir)
        proc = subprocess.Popen(
            [
                str(exe),
                "--headless=new",
                "--remote-debugging-port=0",
                f"--user-data-dir={profile}",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--hide-scrollbars",
                f"--window-size={width},{height}",
                "about:blank",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            from websockets.sync.client import connect

            port = _devtools_port(profile, proc, timeout=30)
            with connect(_page_websocket(port, timeout=30), max_size=None, open_timeout=30) as ws:
                _capture(_Cdp(ws), url, out, width, height, scale, wait_for, wait_gone, settle, timeout)
        finally:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=10)
    return Capture(url=url, path=out, width=width, height=height, scale=scale, browser=exe)


def _capture(
    cdp: _Cdp,
    url: str,
    out: Path,
    width: int,
    height: int,
    scale: float,
    wait_for: str | None,
    wait_gone: str | None,
    settle: float,
    timeout: float,
) -> None:
    cdp.call(
        "Emulation.setDeviceMetricsOverride",
        width=width,
        height=height,
        deviceScaleFactor=scale,
        mobile=False,
    )
    cdp.call("Page.navigate", url=url)
    deadline = time.monotonic() + timeout
    ready = "document.readyState === 'complete'"
    if wait_for:
        ready += f" && document.querySelector({json.dumps(wait_for)}) !== null"
    if wait_gone:
        ready += f" && document.querySelector({json.dumps(wait_gone)}) === null"
    while not cdp.evaluate(ready):
        if time.monotonic() > deadline:
            what = " and ".join(
                ["page load"]
                + ([f"{wait_for!r} to appear"] if wait_for else [])
                + ([f"{wait_gone!r} to disappear"] if wait_gone else [])
            )
            raise ProviderUnavailableError(f"timed out after {timeout:.0f}s waiting for {what} at {url}")
        time.sleep(0.2)
    time.sleep(settle)
    shot = cdp.call("Page.captureScreenshot", timeout=timeout, format="png")
    out.write_bytes(base64.b64decode(shot["data"]))
