"""Decision endpoints: questions tasks wait on, with options, and the user's answers."""

from __future__ import annotations

from typing import Any, Literal

from fastapi import APIRouter, Query

from workgraph.api.deps import AppDep, CtxDep
from workgraph.core.models.schemas import (
    DecisionChoose,
    DecisionOut,
    DecisionRequest,
    DecisionWithdraw,
    DecisionWithTaskOut,
)

router = APIRouter(prefix="/api/v1/ws/{ws}")


@router.get("/decisions")
def list_decisions(
    app: AppDep,
    ctx: CtxDep,
    ws: str,
    status: Literal["open", "decided", "withdrawn", "all"] = "open",
    limit: int = Query(200, ge=1, le=500),
) -> dict[str, list[DecisionWithTaskOut]]:
    return {"items": app.decisions.list(ctx, ws, status, limit)}


@router.get("/decisions/{decision_id}", response_model=DecisionWithTaskOut)
def get_decision(app: AppDep, ctx: CtxDep, ws: str, decision_id: str) -> Any:
    return app.decisions.get(ctx, ws, decision_id)


@router.get("/tasks/{ref}/decisions")
def task_decisions(app: AppDep, ctx: CtxDep, ws: str, ref: str) -> dict[str, list[DecisionOut]]:
    return {"items": app.decisions.for_task(ctx, ws, ref)}


@router.post("/tasks/{ref}/decisions", response_model=DecisionOut, status_code=201)
def request_decision(app: AppDep, ctx: CtxDep, ws: str, ref: str, body: DecisionRequest) -> Any:
    return app.decisions.request(ctx, ws, ref, body.question, body.options)


@router.post("/decisions/{decision_id}/choose", response_model=DecisionOut)
def choose_decision(app: AppDep, ctx: CtxDep, ws: str, decision_id: str, body: DecisionChoose) -> Any:
    return app.decisions.choose(
        ctx, ws, decision_id, option_id=body.option_id, other_text=body.other_text, note=body.note
    )


@router.post("/decisions/{decision_id}/withdraw", response_model=DecisionOut)
def withdraw_decision(
    app: AppDep, ctx: CtxDep, ws: str, decision_id: str, body: DecisionWithdraw | None = None
) -> Any:
    return app.decisions.withdraw(ctx, ws, decision_id, (body or DecisionWithdraw()).reason)
