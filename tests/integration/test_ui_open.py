from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest
from typer.testing import CliRunner

from workgraph.cli.commands import ui
from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.demo.scenario import _free_port


class _Health(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        body = b'{"status": "ok"}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        pass


def _serve_health(port: int) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer(("127.0.0.1", port), _Health)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


@pytest.fixture
def opened(monkeypatch) -> list[str]:
    urls: list[str] = []
    monkeypatch.setattr(ui.webbrowser, "open", lambda url, *a, **k: urls.append(url) or True)
    return urls


def _invoke(*args: str) -> tuple[int, str]:
    STATE.json = False
    STATE.home = None
    try:
        result = CliRunner().invoke(cli, list(args), catch_exceptions=False)
    finally:
        STATE.close()
    return result.exit_code, result.output


def _invoke_json(*args: str) -> tuple[int, dict]:
    STATE.json = True
    STATE.home = None
    try:
        result = CliRunner().invoke(cli, list(args), catch_exceptions=False)
    finally:
        STATE.close()
        STATE.json = False
    return result.exit_code, json.loads(result.output)


def test_open_builds_the_page_url_and_opens_it(opened):
    port = _free_port()
    server = _serve_health(port)
    try:
        base = f"http://127.0.0.1:{port}"
        code, out = _invoke_json("ui", "open", "/decisions?status=open", "--url", base, "--print")
        assert code == 0
        assert out == {"url": f"{base}/decisions?status=open", "started": False}
        code, out = _invoke_json("ui", "open", "tasks/WG-12", "--url", base)
        assert code == 0
        assert out["url"] == f"{base}/tasks/WG-12"
        assert opened == [f"{base}/tasks/WG-12"]
        code, text = _invoke("ui", "open", "--url", base + "/", "--print")
        assert code == 0
        assert text.strip() == f"{base}/"
    finally:
        server.shutdown()
        server.server_close()


def test_open_does_not_start_a_server_when_one_answers(opened, monkeypatch):
    port = _free_port()
    server = _serve_health(port)

    def must_not_start(*args, **kwargs):
        raise AssertionError("a server was started although one answers")

    monkeypatch.setattr(ui, "_start_server", must_not_start)
    try:
        code, out = _invoke_json("ui", "open", "graph", "--url", f"http://127.0.0.1:{port}")
        assert code == 0
        assert out["started"] is False
        assert opened == [f"http://127.0.0.1:{port}/graph"]
    finally:
        server.shutdown()
        server.server_close()


def test_open_with_no_start_fails_clearly_when_nothing_answers(opened, monkeypatch):
    port = _free_port()
    monkeypatch.setattr(ui, "_start_server", lambda *a, **k: pytest.fail("must not start a server"))
    code, out = _invoke_json("ui", "open", "decisions", "--no-start", "--url", f"http://127.0.0.1:{port}")
    assert code == 1
    assert "workgraph serve" in out["error"]["message"]
    assert opened == []


def test_open_with_an_url_never_starts_a_server(opened, monkeypatch):
    port = _free_port()
    monkeypatch.setattr(ui, "_start_server", lambda *a, **k: pytest.fail("must not start a server"))
    code, out = _invoke_json("ui", "open", "--url", f"http://127.0.0.1:{port}")
    assert code == 1
    assert "workgraph serve" in out["error"]["message"]
    assert opened == []


class _FakeProcess:
    def poll(self):
        return None


def test_open_starts_a_server_and_waits_until_it_answers(opened, monkeypatch, home):
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
    port = _free_port()
    servers: list[ThreadingHTTPServer] = []

    def fake_start(log):
        # stands in for the detached `workgraph serve`: something starts answering on the port
        servers.append(_serve_health(port))
        return _FakeProcess()

    monkeypatch.setattr(ui, "_start_server", fake_start)
    monkeypatch.setattr(ui, "_base_url", lambda url: f"http://127.0.0.1:{port}/")
    try:
        code, out = _invoke_json("ui", "open", "decisions", "--timeout", "10")
        assert code == 0
        assert out == {"url": f"http://127.0.0.1:{port}/decisions", "started": True}
        assert opened == [f"http://127.0.0.1:{port}/decisions"]
        assert len(servers) == 1
    finally:
        for server in servers:
            server.shutdown()
            server.server_close()
