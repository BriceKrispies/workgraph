"""Record the HTTP surfaces that sit outside the OpenAPI operations (task CF-04).

Writes four files under ``contracts/http/``:

* ``loopback.json``     loopback clients against ``workgraph serve --local`` with no web build
* ``loopback-ui.json``  the same server with a built web UI (``tools/contracts/fixtures/web``)
* ``lan.json``          a device on the local network: unpaired, paired, outside the LAN, locked out
* ``sse.json``          the event stream: domain events and a queue snapshot

Every exchange records the request (method, path, the headers sent, body) and the response (status,
an allow-listed set of headers, body). The OpenAPI document is not repeated: it is checked for
equality with ``contracts/openapi.json`` and recorded as a reference to that file.

Values that differ between runs are replaced with placeholders (section 3.4 of
``docs/rust-rewrite.md``): the API token, the device cookie, ports, the temporary home, the OS user
name, UUIDs, timestamps and MCP session ids.

The servers run from a fresh ``WORKGRAPH_HOME`` in a temporary directory, on free loopback ports.
The live Workgraph home is never touched.

Usage::

    uv run python tools/contracts/record_http.py           # rewrite the recordings
    uv run python tools/contracts/record_http.py --check   # exit 1 if any recording differs
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import re
import socket
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import httpx

ROOT = Path(__file__).resolve().parents[2]
HTTP_DIR = ROOT / "contracts" / "http"
OPENAPI_FILE = ROOT / "contracts" / "openapi.json"
WEB_FIXTURE = Path(__file__).resolve().parent / "fixtures" / "web"

PAIRING_CODE = "ABCD-EFGH"
LAN_ADDRESS = "192.168.1.10"
DEVICE_ADDRESS = "192.168.1.50"
FOREIGN_ADDRESS = "8.8.8.8"
SSE_PATH = "/api/v1/ws/default/events/stream"
FORM = {"content-type": "application/x-www-form-urlencoded"}
JSON_BODY = {"content-type": "application/json"}

# Headers worth recording: the security headers (O07), the auth and redirect headers, the MCP
# session and the content type. Everything else (date, server, length, transfer) varies or is
# transport noise.
RECORDED_HEADERS = (
    "allow",
    "cache-control",
    "content-type",
    "location",
    "mcp-session-id",
    "referrer-policy",
    "set-cookie",
    "www-authenticate",
    "x-accel-buffering",
    "x-content-type-options",
    "x-frame-options",
)

LOOPBACK_STATE = "loopback, web UI not built"
LOOPBACK_UI_STATE = "loopback, web UI built (fixture)"
UNPAIRED_STATE = "LAN device, not paired"
PAIRED_STATE = "LAN device, paired"
FOREIGN_STATE = "client outside the private networks"
LOCKED_STATE = "LAN device after ten wrong codes"

UUID_RE = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
TIMESTAMP_RE = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})?")
COOKIE_RE = re.compile(r"workgraph_device=([^;]+)")

LAUNCHER = (
    "import sys\n"
    "from pathlib import Path\n"
    "import workgraph.api.app as web\n"
    "from workgraph.cli.entry import main\n"
    "WEB_DIR = Path(sys.argv[1])\n"
    "web.static_dir = lambda: WEB_DIR\n"
    "sys.argv = ['workgraph', *sys.argv[2:]]\n"
    "main()\n"
)


class Redactor:
    """Literal and pattern replacements applied to every recorded string."""

    def __init__(self, home: Path) -> None:
        self.literals: list[tuple[str, str]] = [
            (str(home), "<home>"),
            (home.as_posix(), "<home>"),
            (getpass.getuser(), "<user>"),
        ]

    def add(self, literal: str, placeholder: str) -> None:
        self.literals.append((literal, placeholder))
        self.literals.sort(key=lambda pair: -len(pair[0]))

    def text(self, text: str) -> str:
        for literal, placeholder in self.literals:
            if literal:
                text = text.replace(literal, placeholder)
        text = UUID_RE.sub("<uuid>", text)
        return TIMESTAMP_RE.sub("<ts>", text)

    def scrub(self, value: Any) -> Any:
        if isinstance(value, str):
            return self.text(value)
        if isinstance(value, list):
            return [self.scrub(item) for item in value]
        if isinstance(value, dict):
            return {key: self.scrub(item) for key, item in value.items()}
        return value


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def _env(home: Path) -> dict[str, str]:
    return {**os.environ, "WORKGRAPH_HOME": str(home), "PYTHONIOENCODING": "utf-8"}


def _run_cli(home: Path, *args: str) -> None:
    subprocess.run(
        [sys.executable, "-m", "workgraph", *args],
        cwd=ROOT,
        env=_env(home),
        check=True,
        capture_output=True,
        timeout=120,
    )


def _token(home: Path) -> str:
    return (home / "config" / "api-token").read_text(encoding="utf-8").strip()


def _start_server(home: Path, port: int, *, web_dir: Path | None = None) -> subprocess.Popen[bytes]:
    serve = ["serve", "--local", "--no-worker", "--port", str(port)]
    if web_dir is None:
        argv = [sys.executable, "-m", "workgraph", *serve]
    else:
        argv = [sys.executable, "-c", LAUNCHER, str(web_dir), *serve]
    log = (home.parent / f"serve-{port}.log").open("wb")
    process = subprocess.Popen(argv, cwd=ROOT, env=_env(home), stdout=log, stderr=subprocess.STDOUT)
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"workgraph serve exited early; see {log.name}")
        try:
            if httpx.get(f"http://127.0.0.1:{port}/api/v1/health", timeout=2).status_code == 200:
                return process
        except httpx.TransportError:
            time.sleep(0.2)
    process.kill()
    raise RuntimeError(f"workgraph serve on port {port} did not become ready")


def _stop_server(process: subprocess.Popen[bytes]) -> None:
    process.terminate()
    try:
        process.wait(timeout=20)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def _recorded_headers(response: httpx.Response) -> dict[str, str]:
    return {name: response.headers[name] for name in RECORDED_HEADERS if name in response.headers}


def _body(response: httpx.Response) -> Any:
    if "application/json" in response.headers.get("content-type", ""):
        return response.json()
    return response.text


def _exchange(
    client: httpx.Client,
    *,
    rid: str,
    state: str,
    case: str,
    method: str,
    path: str,
    headers: dict[str, str] | None = None,
    content: str | None = None,
) -> dict[str, Any]:
    sent = {key.lower(): value for key, value in (headers or {}).items()}
    response = client.request(method, path, headers=sent, content=content)
    body = _body(response)
    if rid == "X1" and response.status_code == 200:
        expected = json.loads(OPENAPI_FILE.read_text(encoding="utf-8"))
        if body != expected:
            raise RuntimeError("served OpenAPI document differs from contracts/openapi.json")
        body = {"$ref": "contracts/openapi.json"}
    recorded_headers = _recorded_headers(response)
    if "mcp-session-id" in recorded_headers:
        recorded_headers["mcp-session-id"] = "<session>"
    return {
        "id": rid,
        "state": state,
        "case": case,
        "request": {"method": method, "path": path, "headers": sent, "body": content},
        "response": {"status": response.status_code, "headers": recorded_headers, "body": body},
    }


def _sse_event(name: str | None, seq: str | None, data: list[str]) -> dict[str, Any]:
    raw = "\n".join(data)
    try:
        payload: Any = json.loads(raw)
    except json.JSONDecodeError:
        payload = raw
    event: dict[str, Any] = {"event": name or "message", "data": payload}
    if seq is not None:
        event["id"] = seq
    return event


def _sse_events(lines: Iterator[str]) -> Iterator[dict[str, Any]]:
    name: str | None = None
    seq: str | None = None
    data: list[str] = []
    for line in lines:
        if line == "":
            if name is not None or data:
                yield _sse_event(name, seq, data)
            name, seq, data = None, None, []
        elif line.startswith("event:"):
            name = line[len("event:") :].strip()
        elif line.startswith("id:"):
            seq = line[len("id:") :].strip()
        elif line.startswith("data:"):
            data.append(line[len("data:") :].lstrip())


def _loopback_exchanges(client: httpx.Client, token: str) -> list[dict[str, Any]]:
    auth = {"authorization": f"Bearer {token}"}
    rows = [
        ("X1", "authorized", "GET", "/api/openapi.json", auth, None),
        ("X1", "no token", "GET", "/api/openapi.json", {}, None),
        ("X2", "authorized", "GET", "/api/docs", auth, None),
        ("X2", "no token", "GET", "/api/docs", {}, None),
        ("X3", "no token", "GET", "/docs/oauth2-redirect", {}, None),
        ("X4", "loopback client, no pairing needed", "GET", "/pair", {}, None),
        ("X5", "loopback client, no pairing needed", "POST", "/pair", FORM, f"code={PAIRING_CODE}"),
        ("X6", "no token", "POST", "/mcp/", {}, None),
        ("X6", "authorized, body is not JSON-RPC", "POST", "/mcp/", {**auth, **JSON_BODY}, "{}"),
        ("X7", "web UI not built, root", "GET", "/", {}, None),
        ("X7", "web UI not built, deep path", "GET", "/tasks/WG-1", {}, None),
        ("X7", "api path, no token", "GET", "/api/v1/nope", {}, None),
        ("X7", "api path, authorized", "GET", "/api/v1/nope", auth, None),
        ("X7", "mcp-prefixed path, authorized", "GET", "/mcpx", auth, None),
        ("405", "GET-only route, POST", "POST", "/api/v1/health", {}, None),
        ("421", "foreign Host header", "GET", "/pair", {"host": "evil.example"}, None),
        ("422", "invalid query integer", "GET", "/api/v1/ws/default/events?limit=abc", auth, None),
        (
            "403",
            "foreign Origin on a state-changing request",
            "POST",
            "/api/v1/ws/default/tasks",
            {**auth, **JSON_BODY, "origin": "http://evil.example"},
            "{}",
        ),
    ]
    return [
        _exchange(
            client, rid=rid, state=LOOPBACK_STATE, case=case, method=method, path=path, headers=h, content=c
        )
        for rid, case, method, path, h, c in rows
    ]


def _ui_exchanges(client: httpx.Client) -> list[dict[str, Any]]:
    rows = [
        ("X7", "built UI, root", "GET", "/"),
        ("X7", "built UI, static asset", "GET", "/assets/app.js"),
        ("X7", "built UI, deep path", "GET", "/tasks/WG-1"),
        ("X4", "loopback client, built UI", "GET", "/pair"),
    ]
    return [
        _exchange(client, rid=rid, state=LOOPBACK_UI_STATE, case=case, method=method, path=path)
        for rid, case, method, path in rows
    ]


def _sse_recording(client: httpx.Client, red: Redactor) -> dict[str, Any]:
    with client.stream("GET", SSE_PATH, params={"after_seq": 0}) as response:
        events: list[dict[str, Any]] = []
        for event in _sse_events(response.iter_lines()):
            events.append(event)
            if event["event"] == "queue":
                break
        return {
            "id": "O08",
            "state": LOOPBACK_STATE,
            "case": "stream from after_seq 0: domain events, then a queue snapshot",
            "request": {"method": "GET", "path": f"{SSE_PATH}?after_seq=0", "headers": {}, "body": None},
            "response": {
                "status": response.status_code,
                "headers": _recorded_headers(response),
                "events": events,
            },
        }


def _lan_app(wg: Any, lan_port: int) -> Any:
    from workgraph.api.app import create_app
    from workgraph.api.security import LanAccess

    lan = LanAccess(address=LAN_ADDRESS, pairing_code=PAIRING_CODE)
    return create_app(wg, host="127.0.0.1", port=lan_port, with_mcp=False, lan=lan)


def _lan_client(asgi: Any, lan_port: int, address: str) -> httpx.Client:
    """A client that reaches the app as if from ``address`` on the LAN (no socket is opened)."""
    from starlette.testclient import TestClient

    return TestClient(
        asgi,
        base_url=f"http://{LAN_ADDRESS}:{lan_port}",
        follow_redirects=False,
        client=(address, 40000),
    )


def _lan_recordings(wg: Any, lan_port: int, red: Redactor) -> list[dict[str, Any]]:
    """A device pairs, reads, is refused, and a second run locks pairing after ten wrong codes."""
    rows: list[dict[str, Any]] = []

    asgi = _lan_app(wg, lan_port)
    device = _lan_client(asgi, lan_port, DEVICE_ADDRESS)
    rows.append(
        _exchange(
            device, rid="X7", state=UNPAIRED_STATE, case="root redirects to /pair", method="GET", path="/"
        )
    )
    rows.append(
        _exchange(device, rid="X4", state=UNPAIRED_STATE, case="pairing form", method="GET", path="/pair")
    )
    rows.append(
        _exchange(
            device,
            rid="X5",
            state=UNPAIRED_STATE,
            case="wrong code",
            method="POST",
            path="/pair",
            headers=FORM,
            content="code=AAAA-AAAA",
        )
    )
    paired = _exchange(
        device,
        rid="X5",
        state=UNPAIRED_STATE,
        case="correct code sets the device cookie",
        method="POST",
        path="/pair",
        headers=FORM,
        content=f"code={PAIRING_CODE}",
    )
    rows.append(paired)
    cookie = COOKIE_RE.search(paired["response"]["headers"].get("set-cookie", ""))
    if cookie is None:
        raise RuntimeError("pairing did not set the device cookie")
    red.add(cookie.group(1), "<device-cookie>")
    rows.append(
        _exchange(device, rid="X7", state=PAIRED_STATE, case="paired device, root", method="GET", path="/")
    )
    rows.append(
        _exchange(
            device, rid="X4", state=PAIRED_STATE, case="paired device, /pair", method="GET", path="/pair"
        )
    )
    rows.append(
        _exchange(
            device,
            rid="X1",
            state=PAIRED_STATE,
            case="paired device, no API token",
            method="GET",
            path="/api/openapi.json",
        )
    )
    rows.append(
        _exchange(
            device, rid="X6", state=PAIRED_STATE, case="device on the LAN, MCP", method="POST", path="/mcp/"
        )
    )
    device.close()

    foreign = _lan_client(asgi, lan_port, FOREIGN_ADDRESS)
    rows.append(
        _exchange(
            foreign, rid="X4", state=FOREIGN_STATE, case="public address refused", method="GET", path="/pair"
        )
    )
    foreign.close()

    loopback = _lan_client(asgi, lan_port, "127.0.0.1")
    rows.append(
        _exchange(
            loopback,
            rid="X4",
            state="loopback client on a LAN-enabled server",
            case="trusted",
            method="GET",
            path="/pair",
        )
    )
    loopback.close()

    asgi = _lan_app(wg, lan_port)
    device = _lan_client(asgi, lan_port, DEVICE_ADDRESS)
    for attempt in range(1, 11):
        rows.append(
            _exchange(
                device,
                rid="X5",
                state=LOCKED_STATE,
                case=f"wrong code {attempt} of 10",
                method="POST",
                path="/pair",
                headers=FORM,
                content="code=AAAA-AAAA",
            )
        )
    rows.append(
        _exchange(
            device,
            rid="X5",
            state=LOCKED_STATE,
            case="correct code after the lock",
            method="POST",
            path="/pair",
            headers=FORM,
            content=f"code={PAIRING_CODE}",
        )
    )
    device.close()
    return rows


def build_recordings() -> dict[str, bytes]:
    """Return the bytes of every recording, keyed by file name."""
    from workgraph.app import Workgraph
    from workgraph.config.paths import AppPaths

    with tempfile.TemporaryDirectory(prefix="wg-http-") as tmp:
        home = Path(tmp) / "home"
        _run_cli(home, "init")
        token = _token(home)
        red = Redactor(home)
        red.add(token, "<token>")
        auth = {"authorization": f"Bearer {token}"}
        outputs: dict[str, Any] = {}

        plain_port, ui_port, lan_port = _free_port(), _free_port(), _free_port()
        for port in (plain_port, ui_port, lan_port):
            red.add(f":{port}", ":<port>")

        server = _start_server(home, plain_port)
        try:
            client = httpx.Client(base_url=f"http://127.0.0.1:{plain_port}", follow_redirects=False)
            outputs["loopback.json"] = _loopback_exchanges(client, token)
            _run_cli(home, "--json", "task", "create", "HTTP recording event")
            sse_client = httpx.Client(base_url=f"http://127.0.0.1:{plain_port}", headers=auth, timeout=30)
            outputs["sse.json"] = [_sse_recording(sse_client, red)]
            sse_client.close()
            client.close()
        finally:
            _stop_server(server)

        server = _start_server(home, ui_port, web_dir=WEB_FIXTURE)
        try:
            client = httpx.Client(base_url=f"http://127.0.0.1:{ui_port}", follow_redirects=False)
            outputs["loopback-ui.json"] = _ui_exchanges(client)
            client.close()
        finally:
            _stop_server(server)

        wg = Workgraph(AppPaths.resolve(home))
        try:
            outputs["lan.json"] = _lan_recordings(wg, lan_port, red)
        finally:
            wg.close()

        files: dict[str, bytes] = {}
        for name, rows in outputs.items():
            data = red.scrub({"exchanges": rows})
            files[name] = (json.dumps(data, sort_keys=True, indent=2, ensure_ascii=False) + "\n").encode(
                "utf-8"
            )
        return files


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument(
        "--check", action="store_true", help="compare with the committed recordings instead of writing them"
    )
    args = parser.parse_args(argv)
    recordings = build_recordings()
    if args.check:
        stale = [
            name
            for name, data in recordings.items()
            if not (HTTP_DIR / name).is_file() or (HTTP_DIR / name).read_bytes() != data
        ]
        if stale:
            print("HTTP recordings differ: " + ", ".join(stale), file=sys.stderr)
            print("run: uv run python tools/contracts/record_http.py", file=sys.stderr)
            return 1
        print("HTTP recordings match")
        return 0
    HTTP_DIR.mkdir(parents=True, exist_ok=True)
    for name, data in recordings.items():
        (HTTP_DIR / name).write_bytes(data)
        print(f"wrote {HTTP_DIR / name} ({len(data)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
