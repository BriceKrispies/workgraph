"""FastAPI application factory: REST API, Streamable HTTP MCP endpoint and the web UI."""

from __future__ import annotations

import contextlib
from collections.abc import AsyncGenerator
from pathlib import Path
from typing import Any

from fastapi import FastAPI, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import FileResponse, HTMLResponse, JSONResponse, Response
from starlette.types import ASGIApp

from workgraph import __version__
from workgraph.api.security import LocalSecurityMiddleware, security_headers
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
            "<code>pnpm --dir web install</code> and <code>pnpm --dir web build</code>, then reload.</p>"
            "<p>API: <code>/api/v1</code></p></body></html>"
        )
    html = index.read_text(encoding="utf-8")
    meta = f'<meta name="workgraph-token" content="{token}">'
    return html.replace("</head>", f"{meta}</head>", 1)


def create_app(
    app: Workgraph,
    *,
    host: str = "127.0.0.1",
    port: int = 8765,
    with_mcp: bool = True,
    mcp_agent: str = "mcp-http-client",
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

    @api.get("/{path:path}", include_in_schema=False)
    async def spa(path: str) -> Response:
        if path.startswith(("api/", "mcp")):
            return JSONResponse(
                {"error": {"code": "not_found", "message": f"/{path} not found"}}, status_code=404
            )
        candidate = (assets / path).resolve()
        if path and candidate.is_file() and assets.resolve() in candidate.parents:
            return FileResponse(candidate)
        return HTMLResponse(_index_html(token), headers={"Cache-Control": "no-store"})

    return LocalSecurityMiddleware(api, token=token, host=host, port=port)


def jsonable_errors(errors: Any) -> Any:
    from fastapi.encoders import jsonable_encoder

    return jsonable_encoder(errors, custom_encoder={Exception: str})
