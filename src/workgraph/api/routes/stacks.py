"""Stacks: a parent task's child changes integrated as one unit."""

from __future__ import annotations

from typing import Any

from fastapi import APIRouter

from workgraph.api.deps import AppDep, CtxDep
from workgraph.core.models.schemas import EventOut, StackCreate, StackItemAdd, StackOut

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


@router.post("/ws/{ws}/stacks/{ref}/items", response_model=StackOut, status_code=201)
def add_stack_item(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: StackItemAdd) -> Any:
    """Add a child task of the parent to its active stack."""
    return app.stacks.add_item(ctx, ws, ref, body.task)


@router.delete("/ws/{ws}/stacks/{ref}/items/{task}", response_model=StackOut)
def remove_stack_item(app: AppDep, ctx: CtxDep, ws: str, ref: str, task: str) -> Any:
    """Remove a child task from the active stack; a child that is merged or verified cannot leave it."""
    return app.stacks.remove_item(ctx, ws, ref, task)


@router.get("/ws/{ws}/stacks/{ref}/history", response_model=list[EventOut])
def stack_history(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    """The stack's events in order, with the verification runs they name."""
    return app.stacks.history(ctx, ws, ref)


@router.get("/ws/{ws}/tasks/{ref}/stacks", response_model=list[StackOut])
def task_stacks(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    """Every stack the task is an item of, newest first."""
    return app.stacks.containing(ctx, ws, ref)


@router.post("/ws/{ws}/stacks/{ref}/integrate", status_code=202)
def integrate_stack(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    """Queue integration of the stack's ready children; one job per stack repository."""
    return app.stacks.integrate(ctx, ws, ref)


@router.post("/ws/{ws}/stacks/{ref}/verify", status_code=202)
def verify_stack(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> Any:
    """Queue full verification of the stack's integration tip; bisects and ejects on failure."""
    return app.stack_verification.request(ctx, ws, ref)
