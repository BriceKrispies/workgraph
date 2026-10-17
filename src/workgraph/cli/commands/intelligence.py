"""classify and search commands."""

from __future__ import annotations

from typing import Any

import typer

from workgraph.cli.common import STATE, console, emit, handle_errors, kv, short, table, workspace_option, ws
from workgraph.core.models.schemas import TaskFilter
from workgraph.search.service import SearchFilters

classify_app = typer.Typer(help="Task classification, manual confirmation and model evaluation.", no_args_is_help=True)


@classify_app.command("request")
@handle_errors
def classify_request(task: str, provider: str | None = typer.Option(None), model: str | None = typer.Option(None),
                     promote: bool = typer.Option(False, help="Make the result current even from a different model"),
                     now: bool = typer.Option(False, help="Run synchronously instead of queueing a job"),
                     workspace: str | None = workspace_option()) -> None:
    """Classify a task (queued as a background job unless --now)."""
    app = STATE.app()
    if now:
        t = app.tasks.get(STATE.ctx(), ws(workspace), task)
        out: Any = app.classification.run_classification(STATE.ctx(), t.id, provider=provider, model=model, promote=promote)
        emit(out, lambda c: console.print(f"{c.status}: {c.category} (confidence {c.confidence}) via {c.provider}/{c.model}"))
        return
    out = app.classification.request(STATE.ctx(), ws(workspace), task, provider=provider, model=model, promote=promote)
    emit(out, lambda r: console.print(f"queued classification job {r['job_id']}"))


@classify_app.command("show")
@handle_errors
def classify_show(task: str, workspace: str | None = workspace_option()) -> None:
    """Current classification and its history."""
    result = STATE.app().classification.get(STATE.ctx(), ws(workspace), task)

    def human(r: Any) -> None:
        if r.current:
            c = r.current
            kv([("category", c.category), ("subtype", c.subtype), ("domain", c.domain), ("complexity", c.complexity),
                ("confidence", c.confidence), ("source", c.source), ("model", f"{c.provider}/{c.model}"),
                ("prompt/taxonomy", f"{c.prompt_version}/{c.taxonomy_version}"), ("rationale", c.rationale)],
               title=f"{r.task_key} (current)")
        else:
            console.print("no current classification")
        table([(short(h.id), h.source, h.status, h.category, h.provider, h.model, "*" if h.is_current else "",
                h.validation_status) for h in r.history], ["id", "source", "status", "category", "provider", "model",
                                                          "cur", "validation"])
        console.print(f"[dim]{r.note}[/dim]")

    emit(result, human)


@classify_app.command("confirm")
@handle_errors
def classify_confirm(task: str, category: str, note: str = typer.Option(""), complexity: str | None = typer.Option(None),
                     workspace: str | None = workspace_option()) -> None:
    """Record a human classification (authoritative; also added to the gold set)."""
    out = STATE.app().classification.confirm(STATE.ctx(), ws(workspace), task, category=category, note=note,
                                             complexity=complexity)
    emit(out, lambda c: console.print(f"confirmed {c.category}"))


@classify_app.command("promote")
@handle_errors
def classify_promote(classification_id: str, workspace: str | None = workspace_option()) -> None:
    """Make an alternative model classification current."""
    out = STATE.app().classification.promote(STATE.ctx(), ws(workspace), classification_id)
    emit(out, lambda c: console.print(f"{c.category} is now current"))


@classify_app.command("reclassify")
@handle_errors
def classify_reclassify(provider: str | None = typer.Option(None), model: str | None = typer.Option(None),
                        promote: bool = typer.Option(False), category: str | None = typer.Option(None),
                        workspace: str | None = workspace_option()) -> None:
    """Bulk re-classification as a new batch (previous results are kept)."""
    out = STATE.app().classification.reclassify(STATE.ctx(), ws(workspace), provider=provider, model=model,
                                                promote=promote, flt=TaskFilter(category=category, limit=500))
    emit(out, lambda r: console.print(f"batch {r['batch_id']}: {r['tasks']} job(s) queued"))


@classify_app.command("gold")
@handle_errors
def classify_gold(category: str | None = typer.Option(None, help="Add a label with this category"),
                  task: str | None = typer.Option(None), title: str | None = typer.Option(None),
                  workspace: str | None = workspace_option()) -> None:
    """List the gold-standard set, or add a label to it."""
    app = STATE.app()
    if category:
        emit(app.classification.add_gold(STATE.ctx(), ws(workspace), category=category, task_ref=task, title=title),
             lambda r: console.print(f"gold label {r['id']} added"))
        return
    emit(app.classification.gold(STATE.ctx(), ws(workspace)),
         lambda rows: table([(short(g["id"]), g["category"], g["title"][:70]) for g in rows], ["id", "category", "title"]))


@classify_app.command("evaluate")
@handle_errors
def classify_evaluate(provider: str | None = typer.Option(None), model: str | None = typer.Option(None),
                      workspace: str | None = workspace_option()) -> None:
    """Evaluate a provider/model against the gold set (accuracy, per-class metrics, confusion matrix)."""
    result = STATE.app().classification.evaluate(STATE.ctx(), ws(workspace), provider=provider, model=model)

    def human(r: dict[str, Any]) -> None:
        kv([("model", f"{r['provider']}/{r['model']}"), ("samples", r["sample_size"]), ("failures", r["failures"]),
            ("accuracy", r["accuracy"]), ("macro F1", r["macro_f1"])], title="evaluation")
        labels = list(r["confusion"])
        table([(label, *[r["confusion"][label].get(p, 0) for p in labels]) for label in labels],
              ["expected \\ predicted", *labels])

    emit(result, human)


@classify_app.command("evaluations")
@handle_errors
def classify_evaluations(workspace: str | None = workspace_option()) -> None:
    """Past evaluations across models and prompt versions."""
    rows = STATE.app().classification.evaluations(STATE.ctx(), ws(workspace))
    emit(rows, lambda rs: table([(r["provider"], r["model"], r["prompt_version"], r["sample_size"], r["accuracy"],
                                  r["macro_f1"], r["created_at"][:16]) for r in rs],
                                ["provider", "model", "prompt", "n", "accuracy", "macro F1", "when"]))


@classify_app.command("disagreements")
@handle_errors
def classify_disagreements(workspace: str | None = workspace_option()) -> None:
    """Tasks whose classifications disagree across models or with humans."""
    rows = STATE.app().classification.disagreements(STATE.ctx(), ws(workspace))
    emit(rows, lambda rs: table([(r["task_key"], ", ".join(f"{c['category']}({c['source']}:{c['model']})"
                                                           for c in r["classifications"])) for r in rs],
                                ["task", "classifications"]))


@handle_errors
def search_command(
    query: str = typer.Argument(..., help="Search text"),
    mode: str = typer.Option("hybrid", help="hybrid, lexical or semantic"),
    project: str | None = typer.Option(None, "--project", "-p"),
    repository: str | None = typer.Option(None, "--repo", "-r"),
    category: str | None = typer.Option(None),
    status: list[str] = typer.Option(None, "--status", "-s"),
    agent: str | None = typer.Option(None),
    model: str | None = typer.Option(None),
    path: str | None = typer.Option(None, help="Changed file path contains"),
    limit: int = typer.Option(20),
    similar_to: str | None = typer.Option(None, "--similar-to", help="Find tasks similar to this task instead"),
    reindex: bool = typer.Option(False, help="Rebuild embeddings with the configured model (queues jobs)"),
    workspace: str | None = workspace_option(),
) -> None:
    """Search tasks (full-text always; semantic when embeddings exist)."""
    app = STATE.app()
    if reindex:
        emit(app.search.rebuild_embeddings(STATE.ctx(), ws(workspace)), lambda r: console.print(f"queued {r['enqueued']}"))
        return
    if similar_to:
        result = app.search.similar(STATE.ctx(), ws(workspace), similar_to, limit=limit)
        emit(result, lambda r: table([(s["key"], f"{s['score']:.3f}", s["status"], s["category"] or "", s["title"][:60],
                                       len(s["implementation"])) for s in r["similar"]],
                                     ["task", "score", "status", "category", "title", "commits"],
                                     title=f"similar ({r['method']})"))
        return
    result = app.search.search(STATE.ctx(), ws(workspace), query, mode=mode, limit=limit, filters=SearchFilters(
        project=project, repository=repository, category=category, statuses=status or None, agent=agent, model=model,
        changed_path=path))

    def human(r: Any) -> None:
        table([(h.key, f"{h.score:.4f}", h.status, h.title[:60], (h.snippet or "")[:60]) for h in r.hits],
              ["task", "score", "status", "title", "snippet"], title=f"{r.mode} search")
        if r.note:
            console.print(f"[dim]{r.note}[/dim]")

    emit(result, human)
