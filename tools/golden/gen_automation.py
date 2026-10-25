"""Golden vectors for ``workgraph.automation``: cron next times, templates and rule conditions.

Run from the repository root with ``uv run python tools/golden/gen_automation.py``. The script
imports the Python implementation, runs it over curated and generated inputs and writes
``golden/automation/{cron,templates,conditions}.json``. It reads no clock: every reference
time is a fixed ISO string, so two runs produce identical bytes.
"""

from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path
from typing import Any, get_args

from workgraph.automation.cron import Cron
from workgraph.automation.rules import Condition, Operator, evaluate, render
from workgraph.core.errors import ValidationFailed

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "golden" / "automation"
MISSING = object()


def write(name: str, document: dict[str, Any]) -> Path:
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / f"{name}.json"
    text = json.dumps(document, sort_keys=True, indent=1, ensure_ascii=False) + "\n"
    path.write_bytes(text.encode("utf-8"))
    return path


# --------------------------------------------------------------------------- cron

CRON_CASES: list[tuple[str, str, list[str], list[str]]] = [
    # (id, expression, reference moments, moments for matches())
    (
        "weekday-office-hours",
        "*/15 9-17 * * 1-5",
        [
            "2026-10-19T08:59:00+00:00",
            "2026-10-19T17:45:00+00:00",
            "2026-10-23T17:50:00+00:00",
            "2026-10-24T12:00:00+00:00",
            "2026-10-19T08:59:30.123456+00:00",
        ],
        ["2026-10-19T09:00:00+00:00", "2026-10-24T09:00:00+00:00", "2026-10-19T09:07:00+00:00"],
    ),
    (
        "every-minute",
        "* * * * *",
        ["2026-10-25T10:00:00+00:00", "2026-10-25T10:00:59.999999+00:00"],
        ["2026-10-25T10:00:00+00:00"],
    ),
    (
        "every-five-strict-after",
        "*/5 * * * *",
        ["2026-10-25T10:02:00+00:00", "2026-10-25T10:05:00+00:00"],
        ["2026-10-25T10:05:00+00:00", "2026-10-25T10:06:00+00:00"],
    ),
    (
        "list-of-minutes",
        "0,30 * * * *",
        ["2026-10-25T10:30:00+00:00", "2026-10-25T10:29:00+00:00"],
        ["2026-10-25T10:30:00+00:00"],
    ),
    (
        "range-with-step",
        "0-30/10 * * * *",
        ["2026-10-25T10:00:00+00:00", "2026-10-25T10:31:00+00:00"],
        ["2026-10-25T10:20:00+00:00", "2026-10-25T10:25:00+00:00"],
    ),
    (
        "single-value-step-quirk",
        "5/10 * * * *",
        ["2026-10-25T10:00:00+00:00", "2026-10-25T10:05:00+00:00"],
        ["2026-10-25T10:15:00+00:00", "2026-10-25T10:05:00+00:00"],
    ),
    (
        "sunday-midnight",
        "0 0 * * 0",
        ["2026-10-25T00:00:00+00:00", "2026-10-24T23:59:00+00:00"],
        ["2026-11-01T00:00:00+00:00", "2026-10-25T00:00:00+00:00"],
    ),
    (
        "day-or-weekday-classic",
        "0 12 13 * 5",
        ["2026-10-25T10:00:00+00:00", "2026-10-29T12:00:00+00:00"],
        [
            "2026-10-30T12:00:00+00:00",
            "2026-10-13T12:00:00+00:00",
            "2026-10-14T12:00:00+00:00",
        ],
    ),
    (
        "day-of-month-only",
        "0 12 13 * *",
        ["2026-10-25T10:00:00+00:00"],
        ["2026-11-13T12:00:00+00:00", "2026-10-13T12:00:00+00:00"],
    ),
    (
        "month-boundary",
        "0 0 1 * *",
        ["2026-10-31T23:59:59.5+00:00", "2026-11-01T00:00:00+00:00"],
        ["2026-11-01T00:00:00+00:00"],
    ),
    (
        "year-boundary",
        "0 0 1 1 *",
        ["2026-12-31T12:00:00+00:00"],
        ["2027-01-01T00:00:00+00:00"],
    ),
    (
        "last-minute-of-year",
        "59 23 31 12 *",
        ["2026-10-25T10:00:00+00:00"],
        ["2026-12-31T23:59:00+00:00"],
    ),
    (
        "leap-day-within-cap",
        "0 0 29 2 *",
        ["2027-03-01T00:00:00+00:00"],
        ["2028-02-29T00:00:00+00:00", "2028-03-01T00:00:00+00:00"],
    ),
    (
        "leap-day-beyond-cap",
        "0 0 29 2 *",
        ["2026-10-25T00:00:00+00:00"],
        [],
    ),
    (
        "never-february-31",
        "0 0 31 2 *",
        ["2026-10-25T00:00:00+00:00"],
        [],
    ),
    (
        "never-april-june-31",
        "0 0 31 4,6 *",
        ["2026-10-25T00:00:00+00:00"],
        [],
    ),
    (
        "whitespace-tolerant",
        "  0   5 * * *  ",
        ["2026-10-25T00:00:00+00:00", "2026-10-25T05:00:00+00:00"],
        ["2026-10-25T05:00:00+00:00"],
    ),
    (
        "step-zero-is-invalid",
        "*/0 * * * *",
        [],
        [],
    ),
    (
        "minute-out-of-range",
        "61 * * * *",
        [],
        [],
    ),
    (
        "hour-out-of-range",
        "* 24 * * *",
        [],
        [],
    ),
    (
        "day-zero-invalid",
        "* * 0 * *",
        [],
        [],
    ),
    (
        "month-thirteen-invalid",
        "* * * 13 *",
        [],
        [],
    ),
    (
        "weekday-seven-invalid",
        "0 0 * * 7",
        [],
        [],
    ),
    (
        "weekday-eight-invalid",
        "* * * * 8",
        [],
        [],
    ),
    (
        "four-fields",
        "* * * *",
        [],
        [],
    ),
    (
        "six-fields",
        "* * * * * *",
        [],
        [],
    ),
    (
        "non-numeric-field",
        "a * * * *",
        [],
        [],
    ),
    (
        "reversed-range",
        "5-3 * * * *",
        [],
        [],
    ),
    (
        "dangling-range",
        "1- * * * *",
        [],
        [],
    ),
    (
        "negative-step",
        "*/-1 * * * *",
        [],
        [],
    ),
    (
        "non-numeric-step",
        "*/x * * * *",
        [],
        [],
    ),
    (
        "empty-list-item",
        "1,,2 * * * *",
        [],
        [],
    ),
    (
        "signed-value",
        "+5 * * * *",
        [],
        [],
    ),
    (
        "star-with-range-suffix",
        "*-5 * * * *",
        [],
        [],
    ),
]


def cron_vector(case_id: str, expression: str, references: list[str], moments: list[str]) -> dict[str, Any]:
    try:
        cron = Cron(expression)
    except ValidationFailed as error:
        return {
            "id": case_id,
            "expression": expression,
            "parse": {"ok": False, "error": str(error)},
            "references": [],
            "matches": [],
        }
    results = []
    for reference in references:
        moment = datetime.fromisoformat(reference)
        try:
            nxt = cron.next_after(moment).isoformat()
            results.append({"reference": reference, "next_after": nxt, "error": None})
        except ValidationFailed as error:
            results.append({"reference": reference, "next_after": None, "error": str(error)})
    matches = [
        {"moment": moment_text, "result": cron.matches(datetime.fromisoformat(moment_text))}
        for moment_text in moments
    ]
    return {
        "id": case_id,
        "expression": expression,
        "parse": {"ok": True, "error": None},
        "references": results,
        "matches": matches,
    }


def build_cron() -> dict[str, Any]:
    return {
        "description": "Cron.next_after and Cron.matches for 5-field UTC expressions; "
        "fixed reference times; errors are the Python ValidationFailed messages.",
        "cases": [cron_vector(*case) for case in CRON_CASES],
    }


# --------------------------------------------------------------------------- templates

LONG = "x" * 600
TEMPLATE_CASES: list[tuple[str, str, dict[str, Any], int | None]] = [
    ("no-placeholders", "plain text", {}, None),
    ("empty-template", "", {"task": {"key": "WG-1"}}, None),
    (
        "notify-title",
        "New bug {{task.key}}: {{task.title}}",
        {"task": {"key": "WG-12", "title": "Crash on save"}},
        None,
    ),
    (
        "scheduled-tick",
        "tick {{input.scheduled_for}}",
        {"input": {"scheduled_for": "2026-10-25T00:00:00+00:00"}},
        None,
    ),
    ("inner-spaces", "{{   task.key   }}", {"task": {"key": "WG-7"}}, None),
    ("missing-path", "[{{task.nope}}]", {"task": {}}, None),
    ("null-value", "[{{task.assignee}}]", {"task": {"assignee": None}}, None),
    ("zero-value", "{{n}}", {"n": 0}, None),
    ("false-value", "{{flag}}", {"flag": False}, None),
    ("true-value", "{{flag}}", {"flag": True}, None),
    ("float-value", "{{x}}", {"x": 1.5}, None),
    ("float-integral", "{{x}}", {"x": 2.0}, None),
    ("list-value", "{{task.tags}}", {"task": {"tags": ["a", "b"]}}, None),
    ("dict-value", "{{task}}", {"task": {"k": 1}}, None),
    ("path-through-scalar", "{{task.key.more}}", {"task": {"key": "WG-1"}}, None),
    ("path-through-list-index", "{{task.tags.0}}", {"task": {"tags": ["a"]}}, None),
    ("path-through-null", "{{task.parent.key}}", {"task": {"parent": None}}, None),
    ("uppercase-not-placeholder", "{{Task.key}}", {"Task": {"key": "x"}, "task": {"key": "y"}}, None),
    ("single-braces-literal", "{task.key}", {"task": {"key": "WG-1"}}, None),
    ("leading-digit-literal", "{{1x}}", {"1x": "y"}, None),
    ("digits-in-path", "{{task.key2}}", {"task": {"key2": "ok"}}, None),
    ("unicode-value", "{{task.title}}", {"task": {"title": "café 日本 🚀"}}, None),
    ("value-not-reexpanded", "{{task.title}}", {"task": {"title": "{{task.key}}", "key": "WG-9"}}, None),
    ("value-is-code-text", "{{task.title}}", {"task": {"title": "__import__('os')"}}, None),
    ("adjacent-placeholders", "{{a}}{{b}}", {"a": "1", "b": "2"}, None),
    ("newline-value", "{{t}}", {"t": "a\nb"}, None),
    ("default-limit-truncates", "{{t}}", {"t": LONG}, None),
    ("explicit-limit-100", "{{t}}", {"t": LONG}, 100),
    ("explicit-limit-300-short", "{{t}}", {"t": "short"}, 300),
    ("limit-zero", "{{t}}", {"t": "abc"}, 0),
    ("limit-applies-to-values-only", "{{t}}" + LONG, {"t": "ok"}, 10),
    ("name-and-title-limit-300", "{{t}}", {"t": LONG}, 300),
    ("description-limit-5000", "{{t}}", {"t": LONG}, 5000),
]


def build_templates() -> dict[str, Any]:
    cases = []
    for case_id, template, context, limit in TEMPLATE_CASES:
        output = render(template, context) if limit is None else render(template, context, limit)
        cases.append(
            {
                "id": case_id,
                "template": template,
                "context": context,
                "limit": limit,
                "output": output,
            }
        )
    return {
        "description": "render(template, context, limit) with the default limit of 500 unless "
        "a limit is given; placeholders are {{path}} with lowercase dotted paths.",
        "cases": cases,
    }


# --------------------------------------------------------------------------- conditions

ACTUALS: list[tuple[str, dict[str, Any]]] = [
    ("missing", {}),
    ("null", {"v": None}),
    ("zero", {"v": 0}),
    ("one", {"v": 1}),
    ("one-float", {"v": 1.0}),
    ("true", {"v": True}),
    ("false", {"v": False}),
    ("empty-string", {"v": ""}),
    ("abc", {"v": "abc"}),
    ("crash-42", {"v": "Crash 42"}),
    ("ab", {"v": "ab"}),
    ("empty-list", {"v": []}),
    ("list-a", {"v": ["a"]}),
    ("list-12", {"v": [1, 2]}),
    ("empty-dict", {"v": {}}),
    ("dict-k", {"v": {"k": 1}}),
]
EXPECTEDS: list[tuple[str, Any]] = [
    ("null", None),
    ("zero", 0),
    ("one", 1),
    ("a", "a"),
    ("ab", "ab"),
    ("crash", "crash"),
    ("list-a", ["a"]),
    ("list-1", [1]),
    ("regex-crash", "(?i)crash"),
    ("regex-unclosed", "[unclosed"),
    ("int-12", 12),
    ("dict-k", {"k": 1}),
]

HAND_CASES: list[tuple[str, Any, dict[str, Any]]] = [
    ("none-condition", None, {}),
    ("empty-dict-condition", {}, {}),
    ("empty-all", {"all": []}, {}),
    ("empty-any", {"any": []}, {}),
    ("not-empty-all", {"not": {"all": []}}, {}),
    (
        "shorthand-predicate",
        {"field": "task.priority", "op": "in", "value": ["high"]},
        {"task": {"priority": "high"}},
    ),
    ("both-all-and-any", {"all": [{"field": "a", "op": "exists"}], "any": []}, {"a": 1}),
    ("unknown-key", {"field": "a", "op": "exists", "bogus": 1}, {"a": 1}),
    ("uppercase-field", {"field": "Task.x", "op": "exists"}, {}),
    (
        "trace-example-all",
        {
            "all": [
                {"field": "task.priority", "op": "in", "value": ["high", "critical"]},
                {"not": {"field": "task.tags", "op": "contains", "value": "skip"}},
                {
                    "any": [
                        {"field": "task.title", "op": "matches", "value": "(?i)crash"},
                        {"field": "classification.category", "op": "eq", "value": "bug_fix"},
                    ]
                },
            ]
        },
        {"task": {"priority": "high", "tags": [], "title": "App crash on start"}},
    ),
    (
        "trace-example-low-priority",
        {
            "all": [
                {"field": "task.priority", "op": "in", "value": ["high", "critical"]},
                {"not": {"field": "task.tags", "op": "contains", "value": "skip"}},
                {
                    "any": [
                        {"field": "task.title", "op": "matches", "value": "(?i)crash"},
                        {"field": "classification.category", "op": "eq", "value": "bug_fix"},
                    ]
                },
            ]
        },
        {"task": {"priority": "low", "tags": [], "title": "x"}},
    ),
    ("trace-example-missing-exists", {"field": "missing.path", "op": "not_exists"}, {}),
    (
        "all-short-circuits",
        {
            "all": [
                {"field": "a", "op": "eq", "value": 1},
                {"field": "b", "op": "eq", "value": 2},
            ]
        },
        {"a": 0, "b": 2},
    ),
    (
        "any-short-circuits",
        {
            "any": [
                {"field": "a", "op": "eq", "value": 1},
                {"field": "b", "op": "eq", "value": 2},
            ]
        },
        {"a": 1, "b": 0},
    ),
    (
        "tags-contains-skip",
        {"field": "task.tags", "op": "contains", "value": "skip"},
        {"task": {"tags": ["skip"]}},
    ),
    ("path-through-list-index", {"field": "task.tags.0", "op": "exists"}, {"task": {"tags": ["a"]}}),
    ("bool-equals-one", {"field": "v", "op": "eq", "value": 1}, {"v": True}),
    ("in-string-expected", {"field": "v", "op": "in", "value": "ab"}, {"v": "a"}),
    ("in-falsy-expected", {"field": "v", "op": "in", "value": 0}, {"v": 0}),
    ("contains-dict-keys", {"field": "v", "op": "contains", "value": "k"}, {"v": {"k": 1}}),
    ("starts-with-int-expected", {"field": "v", "op": "starts_with", "value": 12}, {"v": "123"}),
    ("matches-after-10000", {"field": "v", "op": "matches", "value": "crash"}, {"v": "a" * 10000 + "crash"}),
    ("matches-within-10000", {"field": "v", "op": "matches", "value": "crash"}, {"v": "a" * 9990 + "crash"}),
    ("matches-non-string-pattern", {"field": "v", "op": "matches", "value": 5}, {"v": "5"}),
    ("matches-null-pattern", {"field": "v", "op": "matches", "value": None}, {"v": "5"}),
    ("value-length-500", {"field": "v", "op": "eq", "value": "x" * 500}, {"v": "x" * 500}),
    ("value-length-501", {"field": "v", "op": "eq", "value": "x" * 501}, {"v": "x" * 501}),
    ("unicode-matches", {"field": "v", "op": "matches", "value": "日本"}, {"v": "café 日本 🚀"}),
    ("unknown-operator", {"field": "v", "op": "like", "value": "x"}, {"v": "x"}),
]


def _validate(condition_json: Any) -> tuple[Condition | None, dict[str, Any]]:
    if condition_json is None:
        return None, {"valid": True, "error": None}
    try:
        return Condition.model_validate(condition_json), {"valid": True, "error": None}
    except Exception as error:  # the vector records the failure class
        return None, {"valid": False, "error": type(error).__name__}


def _evaluate(case_id: str, condition_json: Any, context: dict[str, Any]) -> dict[str, Any]:
    condition, validity = _validate(condition_json)
    record: dict[str, Any] = {"id": case_id, "condition": condition_json, "context": context, **validity}
    if condition is None and validity["valid"] is False:
        record.update({"result": None, "trace": None})
        return record
    trace: list[dict[str, Any]] = []
    result = evaluate(condition, context, trace)
    record.update({"result": result, "trace": trace})
    return record


def build_conditions() -> dict[str, Any]:
    cases = [_evaluate(case_id, condition, context) for case_id, condition, context in HAND_CASES]
    operators = list(get_args(Operator))
    for op in operators:
        for actual_label, context in ACTUALS:
            for expected_label, expected in EXPECTEDS:
                condition_json = {"field": "v", "op": op, "value": expected}
                cases.append(_evaluate(f"grid/{op}/{actual_label}/{expected_label}", condition_json, context))
    return {
        "description": "Condition validation (Condition.model_validate) and evaluate(condition, context, "
        "trace) over hand-written cases and every operator against every actual and expected value.",
        "operators": operators,
        "cases": cases,
    }


def main() -> None:
    for name, document in (
        ("cron", build_cron()),
        ("templates", build_templates()),
        ("conditions", build_conditions()),
    ):
        path = write(name, document)
        print(f"{path} {path.stat().st_size} bytes, {len(document['cases'])} cases")


if __name__ == "__main__":
    main()
