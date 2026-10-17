"""Local HTTP security: bearer token, Host validation (DNS rebinding) and Origin checks.

* The server binds to loopback by default; non-loopback binding needs explicit opt-in.
* Every ``/api`` and ``/mcp`` request needs the local API token (``Authorization:
  Bearer``). The bundled UI receives it embedded in ``index.html``, which other
  origins cannot read because no permissive CORS headers are ever sent. The SSE
  stream may pass it as ``?token=`` because ``EventSource`` cannot set headers.
* The ``Host`` header must name an allowed host, defeating DNS-rebinding attacks.
* State-changing requests carrying an ``Origin`` header must come from the server's
  own origin (or an explicitly configured development origin).
"""

from __future__ import annotations

import hmac
import os
from collections.abc import Awaitable, Callable

from starlette.requests import Request
from starlette.responses import JSONResponse, Response
from starlette.types import ASGIApp, Receive, Scope, Send

SAFE_METHODS = {"GET", "HEAD", "OPTIONS"}
PUBLIC_PATHS = {"/api/v1/health"}


def allowed_hosts(host: str, port: int) -> set[str]:
    hosts = {"localhost", "127.0.0.1", "[::1]", host}
    extra = os.environ.get("WORKGRAPH_ALLOWED_HOSTS", "")
    hosts |= {h.strip() for h in extra.split(",") if h.strip()}
    with_ports = {f"{h}:{port}" for h in hosts}
    return hosts | with_ports


def allowed_origins(host: str, port: int) -> set[str]:
    origins = {f"http://{h}:{port}" for h in ("localhost", "127.0.0.1", "[::1]", host)}
    extra = os.environ.get("WORKGRAPH_ALLOWED_ORIGINS", "")
    return origins | {o.strip().rstrip("/") for o in extra.split(",") if o.strip()}


class LocalSecurityMiddleware:
    def __init__(self, app: ASGIApp, *, token: str, host: str, port: int) -> None:
        self.app = app
        self.token = token
        self.hosts = allowed_hosts(host, port)
        self.origins = allowed_origins(host, port)

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return
        request = Request(scope)
        host = (request.headers.get("host") or "").lower()
        if host not in self.hosts:
            await JSONResponse(
                {"error": {"code": "forbidden_host", "message": f"host {host!r} not allowed"}},
                status_code=421,
            )(scope, receive, send)
            return
        path = scope["path"]
        protected = (path.startswith(("/api/", "/mcp"))) and path not in PUBLIC_PATHS
        if protected:
            origin = request.headers.get("origin")
            if origin and request.method not in SAFE_METHODS and origin.rstrip("/") not in self.origins:
                await JSONResponse(
                    {"error": {"code": "forbidden_origin", "message": f"origin {origin} not allowed"}},
                    status_code=403,
                )(scope, receive, send)
                return
            if not self._authorized(request):
                await JSONResponse(
                    {"error": {"code": "unauthorized", "message": "missing or invalid API token"}},
                    status_code=401,
                    headers={"WWW-Authenticate": "Bearer"},
                )(scope, receive, send)
                return
        await self.app(scope, receive, send)

    def _authorized(self, request: Request) -> bool:
        header = request.headers.get("authorization", "")
        supplied = (
            header[7:].strip()
            if header.lower().startswith("bearer ")
            else request.headers.get("x-workgraph-token")
        )
        if not supplied and request.url.path.endswith("/events/stream"):
            supplied = request.query_params.get("token")
        return bool(supplied) and hmac.compare_digest(supplied.encode(), self.token.encode())


Handler = Callable[[Request], Awaitable[Response]]


async def security_headers(request: Request, call_next: Handler) -> Response:
    response = await call_next(request)
    response.headers.setdefault("X-Content-Type-Options", "nosniff")
    response.headers.setdefault("X-Frame-Options", "DENY")
    response.headers.setdefault("Referrer-Policy", "no-referrer")
    if request.url.path.startswith("/api/"):
        response.headers.setdefault("Cache-Control", "no-store")
    return response
