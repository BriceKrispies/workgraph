"""Stacks: a parent task's child changes integrated as one unit."""

from __future__ import annotations

from typing import Any

from fastapi import APIRouter

from workgraph.api.deps import AppDep, CtxDep
from workgraph.core.models.schemas import StackCreate, StackOut

router = APIRouter(prefix="/api/v1")


@router.get("/ws/{ws}/stacks", response_model=list[StackOut])
def list_stacks(app: AppDep, ctx: CtxDep, ws: str, include_inactive: bool = False) -> Any:
    return app.stacks.list(ctx, ws, include_inactive=include_inactive)


@router.post("/ws/{ws}/stacks", response_model=StackOut, status_code=201)
def create_stack(app: AppDep, ctx: CtxDep, ws: str, body: StackCreate) -> Any:
    return app.stacks.create(ctx, ws, body.parent)


@router.get("/ws/{ws}/stacks/{ref}", response_model=StackOut)
def get_stack(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    return app.stacks.get(ctx, ws, ref)


@router.post("/ws/{ws}/stacks/{ref}/integrate", status_code=202)
def integrate_stack(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    """Queue integration of the stack's ready children; one job per stack repository."""
    return app.stacks.integrate(ctx, ws, ref)
