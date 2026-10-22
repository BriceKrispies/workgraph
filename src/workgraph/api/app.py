"""FastAPI application factory: REST API, Streamable HTTP MCP endpoint and the web UI."""

from __future__ import annotations

import contextlib
import html
from collections.abc import AsyncGenerator
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs

from fastapi import FastAPI, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import FileResponse, HTMLResponse, JSONResponse, RedirectResponse, Response
from starlette.types import ASGIApp

from workgraph import __version__
from workgraph.api.security import (
    DEVICE_COOKIE,
    LanAccess,
    LocalSecurityMiddleware,
    device_cookie_value,
    device_paired,
    is_loopback_client,
    security_headers,
)
from workgraph.app import Workgraph
from workgraph.core.errors import WorkgraphError
from workgraph.core.logging import get_logger

log = get_logger("api")


def static_dir() -> Path:
    return Path(__file__).parent / "static"


def _index_html(token: str) -> str:
    index = static_dir() / "index.html"
    if not index.exists():
        return (
            "<!doctype html><html><head><meta charset='utf-8'><title>Workgraph</title></head><body>"
            "<h1>Workgraph API is running</h1><p>The web UI has not been built. Run "
            "<code>bun install --cwd web</code> and <code>bun run --cwd web build</code>, then reload.</p>"
            "<p>API: <code>/api/v1</code></p></body></html>"
        )
    html = index.read_text(encoding="utf-8")
    meta = f'<meta name="workgraph-token" content="{token}">'
    return html.replace("</head>", f"{meta}</head>", 1)


def _pair_html(error: str | None = None) -> str:
    message = f'<p class="error">{html.escape(error)}</p>' if error else ""
    return (
        "<!doctype html><html lang='en'><head><meta charset='utf-8'>"
        "<meta name='viewport' content='width=device-width, initial-scale=1'>"
        "<meta name='color-scheme' content='light dark'><title>Pair device - Workgraph</title>"
        "<style>body{font-family:system-ui,sans-serif;margin:0;padding:24px 16px;display:flex;"
        "justify-content:center}main{width:100%;max-width:360px}h1{font-size:1.4rem}"
        "input,button{font-size:1.2rem;padding:12px;width:100%;box-sizing:border-box;margin-top:12px;"
        "border-radius:8px;border:1px solid #888}input{letter-spacing:.15em;text-transform:uppercase}"
        "button{cursor:pointer;background:#2563eb;color:#fff;border-color:#2563eb}"
        ".error{color:#dc2626}</style></head><body><main><h1>Pair this device</h1>"
        "<p>Enter the pairing code that <code>workgraph serve</code> printed on the computer "
        "running Workgraph.</p>"
        f"{message}<form method='post' action='/pair'>"
        "<input name='code' autocomplete='one-time-code' autocapitalize='characters' "
        "placeholder='XXXX-XXXX' required autofocus><button type='submit'>Pair</button>"
        "</form></main></body></html>"
    )


def create_app(
    app: Workgraph,
    *,
    host: str = "127.0.0.1",
    port: int = 8765,
    with_mcp: bool = True,
    mcp_agent: str = "mcp-http-client",
    lan: LanAccess | None = None,
) -> ASGIApp:
    token = app.api_token()
    mcp_server = None
    if with_mcp:
        from workgraph.mcp.server import build_server

        mcp_server = build_server(app, agent_name=mcp_agent)

    @contextlib.asynccontextmanager
    async def lifespan(_: FastAPI) -> AsyncGenerator[None]:
        if mcp_server is not None:
            async with mcp_server.session_manager.run():
                yield
        else:
            yield

    api = FastAPI(
        title="Workgraph",
        version=__version__,
        lifespan=lifespan,
        docs_url="/api/docs",
        openapi_url="/api/openapi.json",
        redoc_url=None,
    )
    api.state.workgraph = app
    api.middleware("http")(security_headers)

    @api.exception_handler(WorkgraphError)
    async def workgraph_error(_: Request, exc: WorkgraphError) -> JSONResponse:
        return JSONResponse({"error": exc.to_dict()}, status_code=exc.http_status)

    @api.exception_handler(RequestValidationError)
    async def validation_error(_: Request, exc: RequestValidationError) -> JSONResponse:
        return JSONResponse(
            {
                "error": {
                    "code": "validation_failed",
                    "message": "invalid request",
                    "details": {"errors": jsonable_errors(exc.errors())},
                }
            },
            status_code=422,
        )

    from workgraph.api.routes import core, ops, stacks, tasks

    api.include_router(core.router)
    api.include_router(tasks.router)
    api.include_router(stacks.router)
    api.include_router(ops.router)

    if mcp_server is not None:
        from mcp.server.transport_security import TransportSecuritySettings

        mcp_app = mcp_server.streamable_http_app(
            streamable_http_path="/",
            transport_security=TransportSecuritySettings(
                enable_dns_rebinding_protection=True,
                allowed_hosts=[f"{h}:{port}" for h in ("127.0.0.1", "localhost", host)]
                + ["127.0.0.1", "localhost"],
                allowed_origins=[f"http://{h}:{port}" for h in ("127.0.0.1", "localhost", host)],
            ),
            host=host,
        )
        api.mount("/mcp", mcp_app)

    assets = static_dir()

    def trusted_browser(request: Request) -> bool:
        """Loopback browsers get the token as before; other devices only once paired."""
        return lan is None or is_loopback_client(request) or device_paired(request, token)

    @api.get("/pair", include_in_schema=False)
    async def pair_form(request: Request) -> Response:
        if trusted_browser(request):
            return RedirectResponse("/", status_code=303)
        return HTMLResponse(_pair_html(), headers={"Cache-Control": "no-store"})

    @api.post("/pair", include_in_schema=False)
    async def pair_submit(request: Request) -> Response:
        if lan is None:
            return RedirectResponse("/", status_code=303)
        if lan.locked:
            return HTMLResponse(
                _pair_html("Too many wrong codes. Restart workgraph serve to pair again."), status_code=429
            )
        code = parse_qs((await request.body()).decode("utf-8", "replace")).get("code", [""])[0]
        if not lan.try_pair(code):
            log.warning("rejected pairing attempt from %s", request.client.host if request.client else "?")
            return HTMLResponse(_pair_html("That code is not right."), status_code=403)
        log.info("paired device %s", request.client.host if request.client else "?")
        response = RedirectResponse("/", status_code=303)
        response.set_cookie(
            DEVICE_COOKIE,
            device_cookie_value(token),
            max_age=400 * 24 * 3600,
            httponly=True,
            samesite="strict",
        )
        return response

    @api.get("/{path:path}", include_in_schema=False)
    async def spa(path: str, request: Request) -> Response:
        if path.startswith(("api/", "mcp")):
            return JSONResponse(
                {"error": {"code": "not_found", "message": f"/{path} not found"}}, status_code=404
            )
        candidate = (assets / path).resolve()
        if path and candidate.is_file() and assets.resolve() in candidate.parents:
            return FileResponse(candidate)
        if not trusted_browser(request):
            return RedirectResponse("/pair", status_code=303)
        return HTMLResponse(_index_html(token), headers={"Cache-Control": "no-store"})

    return LocalSecurityMiddleware(api, token=token, host=host, port=port, lan=lan)


def jsonable_errors(errors: Any) -> Any:
    from fastapi.encoders import jsonable_encoder

    return jsonable_encoder(errors, custom_encoder={Exception: str})
