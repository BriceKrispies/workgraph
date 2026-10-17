"""Task classification: model inference with full provenance, manual confirmation and evaluation.

Policy for the *current* classification of a task:

1. A manual (human-confirmed) classification is never replaced by a model.
2. A result from the *same* provider+model may replace that model's earlier result
   (e.g. after a material description change).
3. A result from a *different* provider/model is stored as an alternative and only
   becomes current when explicitly promoted. Disagreements are recorded as events.

Classifications are inferences, not facts; every row records provider, model,
prompt and taxonomy versions, input hash, raw output, validation status, errors and
duration.
"""

from __future__ import annotations

import builtins
from collections import Counter, defaultdict
from datetime import datetime
from typing import Any

from pydantic import BaseModel, ValidationError
from sqlalchemy import select
from sqlalchemy.orm import Session

from workgraph.classification.taxonomy import (
    CATEGORIES,
    PROMPT_VERSION,
    SYSTEM_PROMPT,
    TAXONOMY_VERSION,
    TaskClassification,
    build_prompt,
    input_hash,
    output_schema,
)
from workgraph.core.clock import new_id
from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError, ProviderUnavailableError, ValidationFailed, WorkgraphError
from workgraph.core.events.log import record_event
from workgraph.core.models.schemas import TaskFilter
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.integrations.llm.base import InvalidModelOutputError
from workgraph.jobs import queue as jobqueue
from workgraph.persistence.orm import (
    Classification,
    ClassificationEvaluation,
    ClassificationGoldLabel,
    Task,
)
from workgraph.search.indexer import index_task
from workgraph.security.permissions import Permission, require


class ClassificationOut(BaseModel):
    id: str
    task_id: str
    source: str
    status: str
    provider: str | None
    model: str | None
    prompt_version: str | None
    taxonomy_version: str
    input_hash: str
    category: str | None
    subtype: str | None
    domain: str | None
    affected_systems: list[str]
    complexity: str | None
    tags: list[str]
    confidence: float | None
    rationale: str | None
    validation_status: str
    error: str | None
    duration_ms: int | None
    is_current: bool
    confirmed_by: str | None
    batch_id: str | None
    created_by: str
    created_at: datetime

    model_config = {"from_attributes": True}


class TaskClassifications(BaseModel):
    task_id: str
    task_key: str
    current: ClassificationOut | None
    history: list[ClassificationOut]
    disagreement: bool
    note: str = "Model classifications are inferences, not objective facts."


class ClassificationService(Service):
    # ------------------------------------------------------------------ requests

    def request(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        *,
        provider: str | None = None,
        model: str | None = None,
        promote: bool = False,
    ) -> dict[str, Any]:
        require(ctx, Permission.CLASSIFY_REQUEST)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            job = jobqueue.enqueue(
                session,
                "classify_task",
                {
                    "task_id": task.id,
                    "provider": provider,
                    "model": model,
                    "promote": promote,
                    "reason": "requested",
                },
                workspace_id=task.workspace_id,
                task_id=task.id,
                resource_keys=["gpu"],
                max_attempts=4,
                backoff_base_seconds=15,
                correlation_id=ctx.correlation_id,
                created_by=ctx.actor,
                idempotency_key=f"classify:{task.id}:{task.description_hash}:{provider}:{model}:{promote}:{new_id()[:8]}",
            )
            return {"job_id": job.id, "task_id": task.id, "task_key": task.key, "status": job.status}

    def run_classification(
        self,
        ctx: RequestContext,
        task_id: str,
        *,
        provider: str | None = None,
        model: str | None = None,
        promote: bool | None = None,
        batch_id: str | None = None,
    ) -> ClassificationOut:
        with self.db.session() as session:
            task = session.get(Task, task_id)
            if task is None:
                raise NotFoundError(f"task {task_id} not found")
            slug = self.app.workspace_slug(task.workspace_id)
            title, description = task.title, task.description
            criteria = list(task.acceptance_criteria or [])
            tags = [t.tag for t in task.tags]
        selection = self.app.llm.selection("classification", slug)
        provider_name = provider or selection.provider
        model_name = model or selection.model
        digest = input_hash(title, description, criteria)
        with self.db.session() as session:
            existing = session.scalar(
                select(Classification).where(
                    Classification.task_id == task_id,
                    Classification.provider == provider_name,
                    Classification.model == model_name,
                    Classification.prompt_version == PROMPT_VERSION,
                    Classification.input_hash == digest,
                    Classification.status == "succeeded",
                )
            )
            if existing is not None and not promote:
                return ClassificationOut.model_validate(existing)
        llm = self.app.llm.get(provider_name, slug)
        prompt = build_prompt(title, description, criteria, tags)
        row_kwargs: dict[str, Any] = {
            "task_id": task_id,
            "source": "model",
            "provider": provider_name,
            "model": model_name,
            "prompt_version": PROMPT_VERSION,
            "taxonomy_version": TAXONOMY_VERSION,
            "input_hash": digest,
            "batch_id": batch_id,
            "created_by": ctx.actor,
        }
        try:
            response = llm.generate_json(
                model_name, system=SYSTEM_PROMPT, prompt=prompt, schema=output_schema()
            )
        except ProviderUnavailableError as exc:
            self._store_failure(ctx, row_kwargs, "provider_unavailable", exc.message)
            raise
        except InvalidModelOutputError as exc:
            out = self._store_failure(ctx, row_kwargs, "invalid_output", exc.message)
            return out
        except WorkgraphError as exc:
            return self._store_failure(ctx, row_kwargs, "error", exc.message)
        try:
            parsed = TaskClassification.model_validate(response.data)
        except ValidationError as exc:
            row_kwargs["raw_output"] = response.data
            row_kwargs["duration_ms"] = response.duration_ms
            return self._store_failure(
                ctx, row_kwargs, "invalid_output", f"output failed schema validation: {exc.errors()[:3]}"
            )
        with self.db.session(write=True) as session:
            task = session.get(Task, task_id)
            assert task is not None
            row = Classification(
                **row_kwargs,
                status="succeeded",
                category=parsed.category,
                subtype=parsed.subtype,
                domain=parsed.domain,
                affected_systems=parsed.affected_systems,
                complexity=parsed.complexity,
                tags=parsed.tags,
                confidence=parsed.confidence,
                rationale=parsed.rationale,
                raw_output=response.data,
                validation_status="valid",
                duration_ms=response.duration_ms,
            )
            session.add(row)
            session.flush()
            self._apply_current_policy(session, ctx, task, row, promote=bool(promote))
            record_event(
                session,
                ctx,
                event_type="classification.completed",
                entity_type="task",
                entity_id=task.id,
                workspace_id=task.workspace_id,
                data={
                    "classification_id": row.id,
                    "category": row.category,
                    "confidence": row.confidence,
                    "provider": provider_name,
                    "model": model_name,
                    "is_current": row.is_current,
                    "prompt_version": PROMPT_VERSION,
                    "taxonomy_version": TAXONOMY_VERSION,
                },
            )
            index_task(session, task.id)
            return ClassificationOut.model_validate(row)

    def _store_failure(
        self, ctx: RequestContext, row_kwargs: dict[str, Any], validation: str, error: str
    ) -> ClassificationOut:
        with self.db.session(write=True) as session:
            row = Classification(
                **row_kwargs, status="failed", validation_status=validation, error=error[:5000]
            )
            session.add(row)
            session.flush()
            task = session.get(Task, row.task_id)
            record_event(
                session,
                ctx,
                event_type="classification.failed",
                entity_type="task",
                entity_id=row.task_id,
                workspace_id=task.workspace_id if task else None,
                data={
                    "classification_id": row.id,
                    "reason": validation,
                    "error": error[:500],
                    "provider": row.provider,
                    "model": row.model,
                },
            )
            return ClassificationOut.model_validate(row)

    def _apply_current_policy(
        self, session: Session, ctx: RequestContext, task: Task, row: Classification, *, promote: bool
    ) -> None:
        current = (
            session.get(Classification, task.current_classification_id)
            if task.current_classification_id
            else None
        )
        replace = False
        reason = ""
        if current is None:
            replace, reason = True, "first classification"
        elif current.source == "manual" or current.confirmed_by:
            replace, reason = False, "manual classification is authoritative"
        elif promote:
            replace, reason = True, "explicitly promoted"
        elif current.provider == row.provider and current.model == row.model:
            replace, reason = True, "same model re-classified"
        else:
            replace, reason = False, "different model; stored as alternative (promote to make current)"
        if current is not None and current.category != row.category:
            record_event(
                session,
                ctx,
                event_type="classification.disagreement",
                entity_type="task",
                entity_id=task.id,
                workspace_id=task.workspace_id,
                data={
                    "current": {
                        "id": current.id,
                        "category": current.category,
                        "source": current.source,
                        "model": current.model,
                    },
                    "new": {"id": row.id, "category": row.category, "model": row.model},
                    "replaced": replace,
                },
            )
        if replace:
            if current is not None:
                current.is_current = False
                record_event(
                    session,
                    ctx,
                    event_type="classification.superseded",
                    entity_type="task",
                    entity_id=task.id,
                    workspace_id=task.workspace_id,
                    data={"previous_id": current.id, "new_id": row.id, "reason": reason},
                )
            row.is_current = True
            task.current_classification_id = row.id
            task.category = row.category
        else:
            row.is_current = False

    def promote(
        self, ctx: RequestContext, workspace: str | None, classification_id: str
    ) -> ClassificationOut:
        require(ctx, Permission.CLASSIFY_CONFIRM)
        with self.db.session(write=True) as session:
            row = session.get(Classification, classification_id)
            if row is None or row.status != "succeeded":
                raise NotFoundError(f"successful classification {classification_id} not found")
            task = session.get(Task, row.task_id)
            assert task is not None
            current = (
                session.get(Classification, task.current_classification_id)
                if task.current_classification_id
                else None
            )
            if current is not None and current.id != row.id:
                current.is_current = False
            row.is_current = True
            task.current_classification_id = row.id
            task.category = row.category
            record_event(
                session,
                ctx,
                event_type="classification.promoted",
                entity_type="task",
                entity_id=task.id,
                workspace_id=task.workspace_id,
                data={"classification_id": row.id, "previous_id": current.id if current else None},
            )
            index_task(session, task.id)
            return ClassificationOut.model_validate(row)

    def confirm(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        *,
        category: str,
        subtype: str | None = None,
        domain: str | None = None,
        complexity: str | None = None,
        tags: builtins.list[str] | None = None,
        note: str = "",
        add_to_gold: bool = True,
    ) -> ClassificationOut:
        """Record a human classification. It becomes current and is never replaced by a model."""
        require(ctx, Permission.CLASSIFY_CONFIRM)
        if category not in CATEGORIES:
            raise ValidationFailed(f"unknown category {category!r}", details={"categories": list(CATEGORIES)})
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            current = (
                session.get(Classification, task.current_classification_id)
                if task.current_classification_id
                else None
            )
            row = Classification(
                task_id=task.id,
                source="manual",
                status="succeeded",
                taxonomy_version=TAXONOMY_VERSION,
                input_hash=input_hash(task.title, task.description, list(task.acceptance_criteria or [])),
                category=category,
                subtype=subtype or (current.subtype if current else None),
                domain=domain or (current.domain if current else None),
                affected_systems=current.affected_systems if current else [],
                complexity=complexity or (current.complexity if current else None),
                tags=tags or [],
                confidence=1.0,
                rationale=note or "confirmed by a human",
                validation_status="manual",
                confirmed_by=ctx.actor,
                created_by=ctx.actor,
            )
            session.add(row)
            session.flush()
            if current is not None:
                current.is_current = False
            row.is_current = True
            task.current_classification_id = row.id
            task.category = category
            if add_to_gold:
                session.add(
                    ClassificationGoldLabel(
                        workspace_id=task.workspace_id,
                        task_id=task.id,
                        title=task.title,
                        description=task.description,
                        category=category,
                        labeled_by=ctx.actor,
                    )
                )
            record_event(
                session,
                ctx,
                event_type="classification.confirmed",
                entity_type="task",
                entity_id=task.id,
                workspace_id=task.workspace_id,
                data={
                    "classification_id": row.id,
                    "category": category,
                    "previous": {"category": current.category, "model": current.model} if current else None,
                },
            )
            index_task(session, task.id)
            return ClassificationOut.model_validate(row)

    def get(self, ctx: RequestContext, workspace: str | None, task_ref: str) -> TaskClassifications:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace) if workspace else None
            task = resolve.task(session, ws_id, task_ref)
            rows = list(
                session.scalars(
                    select(Classification)
                    .where(Classification.task_id == task.id)
                    .order_by(Classification.created_at.desc())
                )
            )
            current = next((r for r in rows if r.id == task.current_classification_id), None)
            categories = {r.category for r in rows if r.status == "succeeded" and r.category}
            return TaskClassifications(
                task_id=task.id,
                task_key=task.key,
                current=ClassificationOut.model_validate(current) if current else None,
                history=[ClassificationOut.model_validate(r) for r in rows],
                disagreement=len(categories) > 1,
            )

    def reclassify(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        flt: TaskFilter | None = None,
        provider: str | None = None,
        model: str | None = None,
        promote: bool = False,
    ) -> dict[str, Any]:
        """Bulk reclassification; prior results are kept (a new batch of rows is appended)."""
        require(ctx, Permission.CLASSIFY_REQUEST)
        batch_id = new_id()
        flt = flt or TaskFilter(limit=500)
        page = self.app.tasks.list(ctx, workspace, flt)
        job_ids = []
        with self.db.session(write=True) as session:
            for task in page.items:
                job = jobqueue.enqueue(
                    session,
                    "classify_task",
                    {
                        "task_id": task.id,
                        "provider": provider,
                        "model": model,
                        "promote": promote,
                        "batch_id": batch_id,
                    },
                    workspace_id=task.workspace_id,
                    task_id=task.id,
                    resource_keys=["gpu"],
                    max_attempts=4,
                    backoff_base_seconds=15,
                    idempotency_key=f"reclassify:{batch_id}:{task.id}",
                    created_by=ctx.actor,
                )
                job_ids.append(job.id)
        return {"batch_id": batch_id, "tasks": len(job_ids), "job_ids": job_ids, "total_matching": page.total}

    # ------------------------------------------------------------------ gold standard & evaluation

    def add_gold(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        category: str,
        task_ref: str | None = None,
        title: str | None = None,
        description: str = "",
    ) -> dict[str, Any]:
        require(ctx, Permission.CLASSIFY_CONFIRM)
        if category not in CATEGORIES:
            raise ValidationFailed(f"unknown category {category!r}")
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            task = resolve.task(session, ws_id, task_ref) if task_ref else None
            if task is None and not title:
                raise ValidationFailed("provide a task or a title for the gold label")
            label = ClassificationGoldLabel(
                workspace_id=ws_id,
                task_id=task.id if task else None,
                title=task.title if task else str(title),
                description=task.description if task else description,
                category=category,
                labeled_by=ctx.actor,
            )
            session.add(label)
            session.flush()
            return {"id": label.id, "title": label.title, "category": category}

    def gold(self, ctx: RequestContext, workspace: str | None) -> builtins.list[dict[str, Any]]:
        require(ctx, Permission.ANALYTICS_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            return [
                {
                    "id": g.id,
                    "task_id": g.task_id,
                    "title": g.title,
                    "category": g.category,
                    "labeled_by": g.labeled_by,
                }
                for g in session.scalars(
                    select(ClassificationGoldLabel).where(ClassificationGoldLabel.workspace_id == ws_id)
                )
            ]

    def evaluate(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        provider: str | None = None,
        model: str | None = None,
    ) -> dict[str, Any]:
        """Run a provider/model over the gold set: accuracy, per-class P/R/F1, confusion matrix."""
        require(ctx, Permission.ANALYTICS_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            slug = self.app.workspace_slug(ws_id)
            labels = [
                (g.id, g.title, g.description, g.category)
                for g in session.scalars(
                    select(ClassificationGoldLabel).where(ClassificationGoldLabel.workspace_id == ws_id)
                )
            ]
        if not labels:
            raise ValidationFailed(
                "no gold-standard labels exist; confirm classifications or add gold labels first"
            )
        selection = self.app.llm.selection("classification", slug)
        provider_name, model_name = provider or selection.provider, model or selection.model
        llm = self.app.llm.get(provider_name, slug)
        results: list[dict[str, Any]] = []
        failures = 0
        for label_id, title, description, expected in labels:
            try:
                response = llm.generate_json(
                    model_name,
                    system=SYSTEM_PROMPT,
                    prompt=build_prompt(title, description, [], []),
                    schema=output_schema(),
                )
                predicted = TaskClassification.model_validate(response.data).category
                results.append(
                    {
                        "label_id": label_id,
                        "title": title,
                        "expected": expected,
                        "predicted": predicted,
                        "correct": predicted == expected,
                    }
                )
            except ProviderUnavailableError:
                raise
            except (WorkgraphError, ValidationError) as exc:
                failures += 1
                results.append(
                    {
                        "label_id": label_id,
                        "title": title,
                        "expected": expected,
                        "predicted": None,
                        "correct": False,
                        "error": str(exc)[:300],
                    }
                )
        metrics = compute_metrics([(r["expected"], r["predicted"]) for r in results])
        with self.db.session(write=True) as session:
            evaluation = ClassificationEvaluation(
                workspace_id=ws_id,
                provider=provider_name,
                model=model_name,
                prompt_version=PROMPT_VERSION,
                taxonomy_version=TAXONOMY_VERSION,
                sample_size=len(results),
                failures=failures,
                accuracy=metrics["accuracy"],
                macro_f1=metrics["macro_f1"],
                per_class=metrics["per_class"],
                confusion=metrics["confusion"],
                results=results,
            )
            session.add(evaluation)
            session.flush()
            record_event(
                session,
                ctx,
                event_type="classification.evaluated",
                entity_type="workspace",
                entity_id=ws_id,
                workspace_id=ws_id,
                data={
                    "evaluation_id": evaluation.id,
                    "provider": provider_name,
                    "model": model_name,
                    "accuracy": metrics["accuracy"],
                    "sample_size": len(results),
                },
            )
            return {
                "id": evaluation.id,
                "provider": provider_name,
                "model": model_name,
                "prompt_version": PROMPT_VERSION,
                "taxonomy_version": TAXONOMY_VERSION,
                "sample_size": len(results),
                "failures": failures,
                **metrics,
                "results": results,
            }

    def evaluations(self, ctx: RequestContext, workspace: str | None) -> builtins.list[dict[str, Any]]:
        require(ctx, Permission.ANALYTICS_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            rows = session.scalars(
                select(ClassificationEvaluation)
                .where(ClassificationEvaluation.workspace_id == ws_id)
                .order_by(ClassificationEvaluation.created_at.desc())
            )
            return [
                {
                    "id": e.id,
                    "provider": e.provider,
                    "model": e.model,
                    "prompt_version": e.prompt_version,
                    "taxonomy_version": e.taxonomy_version,
                    "sample_size": e.sample_size,
                    "failures": e.failures,
                    "accuracy": e.accuracy,
                    "macro_f1": e.macro_f1,
                    "per_class": e.per_class,
                    "confusion": e.confusion,
                    "created_at": e.created_at.isoformat(),
                }
                for e in rows
            ]

    def disagreements(self, ctx: RequestContext, workspace: str | None) -> builtins.list[dict[str, Any]]:
        require(ctx, Permission.ANALYTICS_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            rows = session.execute(
                select(Classification, Task.key)
                .join(Task, Task.id == Classification.task_id)
                .where(Task.workspace_id == ws_id, Classification.status == "succeeded")
            ).all()
            by_task: dict[str, list[tuple[Classification, str]]] = defaultdict(list)
            for row, key in rows:
                by_task[row.task_id].append((row, key))
            out = []
            for task_id, items in by_task.items():
                cats = Counter(r.category for r, _ in items)
                if len(cats) > 1:
                    out.append(
                        {
                            "task_id": task_id,
                            "task_key": items[0][1],
                            "classifications": [
                                {
                                    "id": r.id,
                                    "category": r.category,
                                    "source": r.source,
                                    "provider": r.provider,
                                    "model": r.model,
                                    "is_current": r.is_current,
                                    "confidence": r.confidence,
                                }
                                for r, _ in items
                            ],
                        }
                    )
            return out


def compute_metrics(pairs: builtins.list[tuple[str, str | None]]) -> dict[str, Any]:
    labels = sorted({e for e, _ in pairs} | {p for _, p in pairs if p})
    confusion: dict[str, dict[str, int]] = {e: dict.fromkeys([*labels, "(failed)"], 0) for e in labels}
    for expected, predicted in pairs:
        confusion[expected][predicted or "(failed)"] += 1
    correct = sum(1 for e, p in pairs if e == p)
    per_class: dict[str, dict[str, float | int]] = {}
    f1s = []
    for label in labels:
        tp = confusion.get(label, {}).get(label, 0)
        fp = sum(confusion[e].get(label, 0) for e in confusion if e != label)
        fn = sum(v for k, v in confusion.get(label, {}).items() if k != label)
        precision = tp / (tp + fp) if tp + fp else 0.0
        recall = tp / (tp + fn) if tp + fn else 0.0
        f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
        support = tp + fn
        per_class[label] = {
            "precision": round(precision, 4),
            "recall": round(recall, 4),
            "f1": round(f1, 4),
            "support": support,
        }
        if support:
            f1s.append(f1)
    return {
        "accuracy": round(correct / len(pairs), 4) if pairs else None,
        "macro_f1": round(sum(f1s) / len(f1s), 4) if f1s else None,
        "per_class": per_class,
        "confusion": confusion,
    }
