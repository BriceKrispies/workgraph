"""FastAPI dependencies shared by routers."""

from __future__ import annotations

from typing import Annotated

from fastapi import Depends, Request

from workgraph.app import Workgraph
from workgraph.core.context import RequestContext


def get_app(request: Request) -> Workgraph:
    return request.app.state.workgraph


def get_ctx(request: Request) -> RequestContext:
    """The API token identifies the local operator; an optional header names them."""
    name = request.headers.get("x-workgraph-actor")
    ctx = RequestContext.human(name[:100] if name else None)
    correlation = request.headers.get("x-correlation-id")
    if correlation and len(correlation) <= 64:
        from dataclasses import replace

        ctx = replace(ctx, correlation_id=correlation)
    return ctx


AppDep = Annotated[Workgraph, Depends(get_app)]
CtxDep = Annotated[RequestContext, Depends(get_ctx)]
