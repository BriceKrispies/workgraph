"""Golden vectors for commit attribution (GV-04).

Runs the Python ``workgraph.integrations.git.attribution`` functions over a curated set of
inputs (the cases of ``tests/unit/test_attribution.py``, plus trailer edge cases) and over
generated trailers and messages from a fixed seed, and writes the input/output pairs to
``golden/attribution/``. Output is deterministic: sorted keys, LF line endings, UTF-8.

Usage: ``uv run python tools/golden/gen_attribution.py``
"""

from __future__ import annotations

import json
import random
from pathlib import Path
from typing import Any

from workgraph.integrations.git.attribution import Candidate, message_candidates, trailer_candidates

ROOT = Path(__file__).resolve().parents[2]
OUT_DIR = ROOT / "golden" / "attribution"

GENERATED_TRAILER_CASES = 240
GENERATED_MESSAGE_CASES = 480
SEED = 20251025

TEST_FILE = "tests/unit/test_attribution.py"

# (test function name, message, prefixes, trailers). Each entry is one case of the test file.
CURATED_MESSAGE_CASES: list[tuple[str, str, list[str], list[dict[str, str]]]] = [
    ("test_empty_prefix_list_matches_nothing", "fixes WG-1", [], []),
    ("test_single_prefix_case_insensitive_and_normalised", "fix wg-12 and WG-3", ["WG"], []),
    ("test_single_key_has_full_confidence", "fix wg-12", ["WG"], []),
    ("test_two_prefixes_both_recognised", "closes ABC-7, relates to WG-2", ["ABC", "WG"], []),
    ("test_mixed_case_prefix_is_upper_cased", "see abc-7", ["Abc"], []),
    ("test_longer_prefix_does_not_yield_shorter_match", "ABCD-12 only", ["ABC", "ABCD"], []),
    ("test_shorter_prefix_inside_longer_key_is_not_matched", "XABC-1 only", ["ABC"], []),
    ("test_shorter_prefix_inside_longer_key_is_not_matched", "XABC-1 then ABC-2", ["ABC"], []),
    ("test_lookahead_semantics_follow_the_original_rule", "ABC-12345", ["ABC"], []),
    ("test_lookahead_semantics_follow_the_original_rule", "ABC-12a", ["ABC"], []),
    (
        "test_duplicates_are_collapsed_in_order_of_appearance",
        "ABC-5 then WG-1 then abc-5",
        ["WG", "ABC"],
        [],
    ),
    (
        "test_trailer_lines_are_excluded_from_prose",
        "implement\n\nTask-ID: ABC-7",
        ["ABC"],
        [{"key": "Task-ID", "value": "ABC-7"}],
    ),
    (
        "test_trailer_lines_are_excluded_from_prose",
        "implement\n\nTask-ID: ABC-7\nalso ABC-8",
        ["ABC"],
        [{"key": "Task-ID", "value": "ABC-7"}],
    ),
]

# Hand-written edge cases beyond the test file: boundaries, separators, non-ASCII, empty input.
EXTRA_MESSAGE_CASES: list[tuple[str, str, list[str], list[dict[str, str]]]] = [
    ("extra:empty_message", "", ["WG"], []),
    ("extra:no_key_in_message", "just words", ["WG"], []),
    ("extra:key_at_line_start", "WG-1\nWG-2", ["WG"], []),
    ("extra:key_followed_by_punctuation", "done (WG-9).", ["WG"], []),
    ("extra:digits_before_prefix", "1WG-2", ["WG"], []),
    ("extra:underscore_before_prefix", "_WG-4", ["WG"], []),
    ("extra:underscore_after_number", "WG-4_", ["WG"], []),
    ("extra:leading_zeros_kept", "WG-0005", ["WG"], []),
    ("extra:very_large_number", "WG-99999999999999999999999", ["WG"], []),
    ("extra:prefix_without_number", "WG- and WG-x", ["WG"], []),
    ("extra:dot_in_prefix", "see W.G-3 and WXG-3", ["W.G"], []),
    ("extra:regex_metachar_prefix", "A+B-1 and AAB-1", ["A+B"], []),
    ("extra:space_in_prefix", "A B-1 A B-2", ["A B"], []),
    ("extra:non_ascii_before_key", "é WG-5 ünï WG-6", ["WG"], []),
    ("extra:non_ascii_prefix_text", "naïve WG-7", ["WG"], []),
    ("extra:crlf_lines", "fix\r\nWG-3\r\nTask-ID: WG-3\r\n", ["WG"], [{"key": "Task-ID", "value": "WG-3"}]),
    (
        "extra:trailer_line_with_extra_spaces",
        "x\n  Task-ID: WG-8  \nWG-9",
        ["WG"],
        [{"key": "Task-ID", "value": "WG-8"}],
    ),
    (
        "extra:trailer_text_not_exact_line",
        "Task-ID: WG-8 and WG-9",
        ["WG"],
        [{"key": "Task-ID", "value": "WG-8"}],
    ),
    ("extra:only_empty_prefixes", "WG-1", ["", ""], []),
    ("extra:three_keys_ambiguous", "WG-1 WG-2 WG-3", ["WG"], []),
    ("extra:same_key_twice_is_single", "WG-1 wg-1 WG-1", ["WG"], []),
]

TRAILER_KEYS = [
    "Task-ID",
    "task-id",
    "TASK-ID",
    "Workgraph-Task",
    "wg-task",
    "WG-Task",
    "Co-Authored-By",
    "Reviewed-by",
    "Signed-off-by",
    "Fixes",
]
TRAILER_TOKENS = ["WG-1", "wg-22", "ABC-7", "ABCD-100", "XYZ", "", "7", "WG-", "WG-3,", "é-1"]
TRAILER_SEPARATORS = [",", " ", ", ", "\t", ",,", "  ", " , "]

MESSAGE_FRAGMENTS = [
    "fix",
    "closes",
    "see",
    "relates to",
    "and",
    "then",
    "wip:",
    "(",
    ")",
    ".",
    "é",
    "WG-12",
    "wg-3",
    "WG-0007",
    "ABC-7",
    "abc-22",
    "ABCD-12",
    "XABC-1",
    "ABC-12a",
    "ABC-",
    "12-ABC",
    "WG-99999999999999",
    "ABC-5",
    "ABCD-9",
    "A.B-4",
    "Task-ID: WG-30",
    "Task-ID: ABC-7",
    "Workgraph-Task: WG-31",
    "wg-task: WG-32",
    "Fixes: WG-33",
]
MESSAGE_SEPARATORS = [" ", "  ", "\n", "\n\n", ", ", "\t", "_", "-"]
MESSAGE_PREFIX_POOL = ["WG", "ABC", "ABCD", "Abc", "ABC", "A.B", "A B", "", "wg", "XYZ"]


def _candidate_dict(candidate: Candidate) -> dict[str, Any]:
    return {
        "task_ref": candidate.task_ref,
        "source": candidate.source,
        "confidence": candidate.confidence,
        "evidence": candidate.evidence,
    }


def _message_output(
    message: str, prefixes: list[str], trailers: list[dict[str, str]]
) -> list[dict[str, Any]]:
    return [_candidate_dict(c) for c in message_candidates(message, prefixes, trailers)]


def _trailer_output(trailers: list[dict[str, str]]) -> list[dict[str, Any]]:
    return [_candidate_dict(c) for c in trailer_candidates(trailers)]


def _generated_trailers(rng: random.Random) -> list[list[dict[str, str]]]:
    cases: list[list[dict[str, str]]] = []
    for _ in range(GENERATED_TRAILER_CASES):
        trailers: list[dict[str, str]] = []
        for _ in range(rng.randint(0, 4)):
            key = rng.choice(TRAILER_KEYS)
            tokens = [rng.choice(TRAILER_TOKENS) for _ in range(rng.randint(0, 3))]
            value = ""
            for token in tokens:
                value += token + rng.choice(TRAILER_SEPARATORS)
            if rng.random() < 0.5:
                value = value.rstrip()
            trailers.append({"key": key, "value": value})
        cases.append(trailers)
    return cases


def _generated_messages(rng: random.Random) -> list[tuple[str, list[str], list[dict[str, str]]]]:
    cases: list[tuple[str, list[str], list[dict[str, str]]]] = []
    for _ in range(GENERATED_MESSAGE_CASES):
        trailers: list[dict[str, str]] = []
        if rng.random() < 0.3:
            trailers.append(
                {
                    "key": rng.choice(["Task-ID", "task-id", "WG-Task"]),
                    "value": rng.choice(["WG-30", "ABC-7", "WG-2, ABC-7"]),
                }
            )
        parts: list[str] = []
        for _ in range(rng.randint(0, 9)):
            parts.append(rng.choice(MESSAGE_FRAGMENTS))
            parts.append(rng.choice(MESSAGE_SEPARATORS))
        if trailers and rng.random() < 0.8:
            parts.append("\n\n" + f"{trailers[0]['key']}: {trailers[0]['value']}")
        message = "".join(parts)
        prefixes = rng.sample(MESSAGE_PREFIX_POOL, k=rng.randint(0, 3))
        cases.append((message, prefixes, trailers))
    return cases


def _write(name: str, payload: dict[str, Any]) -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    text = json.dumps(payload, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    with (OUT_DIR / name).open("w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def main() -> None:
    rng = random.Random(SEED)  # noqa: S311 - test-data generation, not security

    trailer_cases: list[dict[str, Any]] = []
    for index, trailers in enumerate(_generated_trailers(rng)):
        trailer_cases.append(
            {
                "id": f"generated:trailer:{index:03d}",
                "origin": f"generated (seed {SEED})",
                "input": {"trailers": trailers},
                "output": _trailer_output(trailers),
            }
        )
    # Curated trailer edge cases, not from the test file.
    curated_trailers = [
        [{"key": "Task-ID", "value": "WG-1, WG-2"}],
        [{"key": "Task-ID", "value": "WG-1\tWG-2  ABC-7"}],
        [{"key": "Task-ID", "value": ""}],
        [{"key": "Task-ID", "value": " , , "}],
        [{"key": "Signed-off-by", "value": "WG-1"}],
        [{"key": "Task-ID", "value": "WG-1"}, {"key": "wg-task", "value": "WG-2"}],
        [],
    ]
    for index, trailers in enumerate(curated_trailers):
        trailer_cases.append(
            {
                "id": f"curated:trailer:{index:03d}",
                "origin": "hand-written trailer edge case",
                "input": {"trailers": trailers},
                "output": _trailer_output(trailers),
            }
        )
    _write("trailers.json", {"function": "trailer_candidates", "cases": trailer_cases})

    message_cases: list[dict[str, Any]] = []
    for name, message, prefixes, trailers in CURATED_MESSAGE_CASES:
        message_cases.append(
            {
                "id": f"{TEST_FILE}::{name}",
                "origin": f"{TEST_FILE}::{name}",
                "input": {"message": message, "prefixes": prefixes, "trailers": trailers},
                "output": _message_output(message, prefixes, trailers),
            }
        )
    for name, message, prefixes, trailers in EXTRA_MESSAGE_CASES:
        message_cases.append(
            {
                "id": name,
                "origin": "hand-written edge case",
                "input": {"message": message, "prefixes": prefixes, "trailers": trailers},
                "output": _message_output(message, prefixes, trailers),
            }
        )
    for index, (message, prefixes, trailers) in enumerate(_generated_messages(rng)):
        message_cases.append(
            {
                "id": f"generated:message:{index:03d}",
                "origin": f"generated (seed {SEED})",
                "input": {"message": message, "prefixes": prefixes, "trailers": trailers},
                "output": _message_output(message, prefixes, trailers),
            }
        )
    _write("message_inference.json", {"function": "message_candidates", "cases": message_cases})


if __name__ == "__main__":
    main()
