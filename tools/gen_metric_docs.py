"""Regenerate the metric reference in docs/analytics.md from the metric registry.

Usage: uv run tools/gen_metric_docs.py
"""

from __future__ import annotations

from pathlib import Path

from workgraph.analytics.metrics import BASE_DIMENSIONS, METRICS

DOC = Path(__file__).resolve().parent.parent / "docs" / "analytics.md"
START = "<!-- metrics:start -->"
END = "<!-- metrics:end -->"


def render() -> str:
    lines = [
        START,
        "",
        "| Metric | Family | Statistic | Unit | Time column | Group by | Definition |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    for m in METRICS:
        dims = ", ".join(sorted(BASE_DIMENSIONS[m.base] | {"period"}))
        caveats = " " + " ".join(f"*{c}*" for c in m.caveats) if m.caveats else ""
        lines.append(
            f"| `{m.id}` | {m.family} | {m.kind} | {m.unit} | `{m.time_column}` | {dims} | "
            f"{m.definition}{caveats} |"
        )
    lines += ["", END]
    return "\n".join(lines)


def main() -> None:
    text = DOC.read_text(encoding="utf-8")
    head, rest = text.split(START, 1)
    _, tail = rest.split(END, 1)
    DOC.write_text(head + render() + tail, encoding="utf-8", newline="\n")
    print(f"updated {DOC}")


if __name__ == "__main__":
    main()
