from __future__ import annotations

import json
from pathlib import Path

import pytest
from typer.testing import CliRunner

from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.core.errors import ProviderUnavailableError, ValidationFailed
from workgraph.core.models.schemas import TaskCreate
from workgraph.demo.scenario import _free_port, _start_api
from workgraph.integrations.browser import BROWSER_ENV, _capture, find_browser, screenshot

STATIC_UI = Path(__file__).parents[2] / "src" / "workgraph" / "api" / "static" / "index.html"


def _invoke(*args: str) -> tuple[int, dict]:
    STATE.json = True
    STATE.home = None
    try:
        result = CliRunner().invoke(cli, list(args), catch_exceptions=False)
    finally:
        STATE.close()
        STATE.json = False
    assert result.output.strip(), f"no output; stderr: {result.stderr}"
    return result.exit_code, json.loads(result.output)


def test_find_browser_prefers_the_environment(tmp_path, monkeypatch):
    fake = tmp_path / "my-chrome.exe"
    fake.write_text("")
    monkeypatch.setenv(BROWSER_ENV, str(fake))
    assert find_browser() == fake
    monkeypatch.setenv(BROWSER_ENV, str(tmp_path / "missing.exe"))
    with pytest.raises(ProviderUnavailableError, match="browser not found"):
        find_browser()


def test_screenshot_without_a_server_fails_clearly(home, monkeypatch, tmp_path):
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
    url = f"http://127.0.0.1:{_free_port()}"
    code, out = _invoke("ui", "screenshot", "graph", "-o", str(tmp_path / "x.png"), "--url", url)
    assert code == 1
    assert "workgraph serve" in out["error"]["message"]
    assert not (tmp_path / "x.png").exists()


@pytest.mark.slow
def test_screenshot_of_the_graph_page(app, human, ws, home, monkeypatch, tmp_path):
    try:
        find_browser()
    except ProviderUnavailableError:
        pytest.skip("no Chrome, Edge or Chromium installed")
    if not STATIC_UI.exists():
        pytest.skip("web UI not built (cd web && npm run build)")
    first = app.tasks.create(human, ws, TaskCreate(title="First"))
    app.tasks.create(human, ws, TaskCreate(title="Second", depends_on=[first.key]))
    port = _free_port()
    thread, server = _start_api(app, port)
    try:
        monkeypatch.setenv("WORKGRAPH_HOME", str(home))
        out = tmp_path / "graph.png"
        code, result = _invoke(
            "ui", "screenshot", "graph?linked=1", "-o", str(out),
            "--url", f"http://127.0.0.1:{port}", "--width", "800", "--height", "600", "--settle", "0.2",
        )  # fmt: skip
    finally:
        server.should_exit = True
        thread.join(timeout=30)
    assert code == 0, result
    assert result["file"] == str(out.resolve())
    data = out.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    width, height = int.from_bytes(data[16:20], "big"), int.from_bytes(data[20:24], "big")
    assert (width, height) == (800, 600)


class _RecordingCdp:
    def __init__(self) -> None:
        self.calls: list[tuple[str, dict]] = []

    def call(self, method: str, **params):
        self.calls.append((method, params))
        return {"data": "iVBORw0KGgo="} if method == "Page.captureScreenshot" else {}

    def evaluate(self, expression: str):
        return True


def test_capture_emulates_a_dark_touch_phone(tmp_path):
    cdp = _RecordingCdp()
    _capture(cdp, "http://x/", tmp_path / "p.png", 390, 844, 3, None, None, 0, 5, theme="dark", mobile=True)
    calls = dict(cdp.calls)
    assert calls["Emulation.setDeviceMetricsOverride"]["mobile"] is True
    assert calls["Emulation.setTouchEmulationEnabled"]["enabled"] is True
    assert calls["Emulation.setEmulatedMedia"]["features"] == [
        {"name": "prefers-color-scheme", "value": "dark"}
    ]


def test_capture_leaves_the_desktop_default_alone(tmp_path):
    cdp = _RecordingCdp()
    _capture(cdp, "http://x/", tmp_path / "p.png", 800, 600, 1, None, None, 0, 5)
    methods = [m for m, _ in cdp.calls]
    assert "Emulation.setEmulatedMedia" not in methods
    assert "Emulation.setTouchEmulationEnabled" not in methods
    assert dict(cdp.calls)["Emulation.setDeviceMetricsOverride"]["mobile"] is False


def test_screenshot_rejects_an_unknown_theme(tmp_path):
    with pytest.raises(ValidationFailed, match="theme"):
        screenshot("http://x/", tmp_path / "p.png", theme="sepia")
