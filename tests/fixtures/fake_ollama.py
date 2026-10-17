"""A real HTTP server implementing the subset of the Ollama API Workgraph uses (test fixture)."""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from workgraph.integrations.llm.heuristic import classify_text, hash_embedding


class FakeOllama:
    def __init__(self) -> None:
        self.models = ["qwen-test:latest", "other-model:latest", "broken-model:latest", "embed-test:latest"]
        self.fail_next = 0  # respond 503 this many times
        self.requests: list[dict[str, Any]] = []
        self.forced_category: str | None = None
        server = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: Any) -> None:
                pass

            def _send(self, status: int, body: dict[str, Any]) -> None:
                data = json.dumps(body).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self) -> None:
                if self.path == "/api/tags":
                    self._send(200, {"models": [{"name": m} for m in server.models]})
                else:
                    self._send(404, {"error": "not found"})

            def do_POST(self) -> None:
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                server.requests.append({"path": self.path, "body": body})
                if server.fail_next > 0:
                    server.fail_next -= 1
                    self._send(503, {"error": "busy"})
                    return
                model = body.get("model", "")
                if model not in server.models and f"{model}:latest" not in server.models:
                    self._send(404, {"error": f"model '{model}' not found, try pulling it first"})
                    return
                if self.path == "/api/chat":
                    if model.startswith("broken-model"):
                        content = "this is not json"
                    else:
                        text = body["messages"][-1]["content"]
                        task = text.split("<task>")[-1].split("</task>")[0]
                        result = classify_text(task)
                        if server.forced_category and model.startswith("other-model"):
                            result["category"] = server.forced_category
                        content = json.dumps(result)
                    self._send(
                        200,
                        {
                            "model": model,
                            "message": {"role": "assistant", "content": content},
                            "done": True,
                            "eval_count": 42,
                        },
                    )
                elif self.path == "/api/embed":
                    self._send(
                        200, {"model": model, "embeddings": [hash_embedding(t) for t in body["input"]]}
                    )
                else:
                    self._send(404, {"error": "unknown endpoint"})

        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)

    @property
    def url(self) -> str:
        host, port = self.httpd.server_address[:2]
        return f"http://{host}:{port}"

    def __enter__(self) -> FakeOllama:
        self.thread.start()
        return self

    def __exit__(self, *exc: object) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()
