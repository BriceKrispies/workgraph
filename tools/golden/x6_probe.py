"""Golden for X6: what the server answers to an authorized POST /mcp/ whose body is not a JSON-RPC
message (WG-3764).

Starts ``workgraph serve --local --no-worker`` from a fresh ``WORKGRAPH_HOME`` in a temporary
directory, on a free loopback port, and sends each body with the same headers as the X6
recording (``authorization`` and ``content-type`` only, no ``accept``). The status, the content
type, whether an ``mcp-session-id`` header came back, and the body text exactly as served are
written to ``crates/wg-mcpspec/tests/golden/x6_bodies.json``.

Run with ``uv run python tools/golden/x6_probe.py`` to rewrite the golden from the Python server.
With ``--rust PATH`` the same probe runs against the Rust ``workgraph`` binary and the answers are
compared with the golden (exit 1 on any difference). The live Workgraph home is never touched.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import httpx

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates" / "wg-mcpspec" / "tests" / "golden" / "x6_bodies.json"

BODIES: list[tuple[str, str]] = [
    ("wrong-typed field", '{"jsonrpc":"1.0","id":1,"method":"x"}'),
    ("empty array", "[]"),
    ("string", '"x"'),
    ("integer", "3"),
    ("null", "null"),
    ("unparseable", "{bad"),
    ("empty body", ""),
]

# The fields compared with the golden. The session id value is not compared, only its presence.
KEYS = ("case", "status", "content_type", "has_session_id", "text")


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def _env(home: Path) -> dict[str, str]:
    return {**os.environ, "WORKGRAPH_HOME": str(home), "PYTHONIOENCODING": "utf-8"}


def _probe(command: list[str]) -> list[dict[str, object]]:
    with tempfile.TemporaryDirectory(prefix="wg-x6-", ignore_cleanup_errors=True) as tmp:
        home = Path(tmp) / "home"
        subprocess.run(
            [*command, "init"],
            cwd=ROOT,
            env=_env(home),
            check=True,
            capture_output=True,
            timeout=120,
        )
        token = (home / "config" / "api-token").read_text(encoding="utf-8").strip()
        port = _free_port()
        server = subprocess.Popen(
            [*command, "serve", "--local", "--no-worker", "--port", str(port)],
            cwd=ROOT,
            env=_env(home),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.monotonic() + 90
            while True:
                if server.poll() is not None:
                    raise RuntimeError("workgraph serve exited early")
                try:
                    if httpx.get(f"http://127.0.0.1:{port}/api/v1/health", timeout=2).status_code == 200:
                        break
                except httpx.TransportError:
                    pass
                if time.monotonic() > deadline:
                    raise RuntimeError("workgraph serve did not become ready")
                time.sleep(0.2)
            cases: list[dict[str, object]] = []
            for name, body in BODIES:
                response = httpx.post(
                    f"http://127.0.0.1:{port}/mcp/",
                    headers={"authorization": f"Bearer {token}", "content-type": "application/json"},
                    content=body.encode("utf-8"),
                )
                cases.append(
                    {
                        "case": name,
                        "body": body,
                        "status": response.status_code,
                        "content_type": response.headers.get("content-type", ""),
                        "has_session_id": "mcp-session-id" in response.headers,
                        "text": response.text,
                    }
                )
            return cases
        finally:
            server.terminate()
            try:
                server.wait(timeout=20)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


def main() -> int:
    parser = argparse.ArgumentParser(description="Probe X6 answers and write or check the golden.")
    parser.add_argument("--rust", type=Path, help="check this Rust workgraph binary against the golden")
    args = parser.parse_args()
    if args.rust:
        cases = _probe([str(args.rust)])
        golden = json.loads(OUT.read_text(encoding="utf-8"))["cases"]
        differences = []
        for want, got in zip(golden, cases, strict=True):
            want_fields = {key: want[key] for key in KEYS}
            got_fields = {key: got[key] for key in KEYS}
            if got_fields != want_fields:
                differences.append(f"{want['case']}: got {got_fields}, wanted {want_fields}")
        for line in differences or ["all answers match the golden"]:
            print(line)
        return 1 if differences else 0
    cases = _probe([sys.executable, "-m", "workgraph"])
    document = {
        "source": "tools/golden/x6_probe.py against workgraph serve --local (Python)",
        "cases": cases,
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    for case in cases:
        print(f"{case['case']}: {case['status']} {case['content_type']} session={case['has_session_id']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
