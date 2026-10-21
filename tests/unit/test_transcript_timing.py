from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta

from workgraph.integrations.claude_code.timing import SLOWEST_CALLS, breakdown, classify
from workgraph.integrations.claude_code.transcript import parse_transcript

BASE = datetime(2026, 10, 14, 8, 0, 0, tzinfo=UTC)


def at(seconds: float) -> str:
    return (BASE + timedelta(seconds=seconds)).isoformat().replace("+00:00", "Z")


def prompt(seconds: float, text: str = "do the work") -> dict:
    return {"type": "user", "timestamp": at(seconds), "message": {"role": "user", "content": text}}


def turn(seconds: float, *uses: dict, message_id: str | None = None, text: str = "ok") -> dict:
    content = [*uses] if uses else [{"type": "text", "text": text}]
    return {
        "type": "assistant",
        "timestamp": at(seconds),
        "message": {"id": message_id or f"msg-{seconds}", "model": "claude-haiku-5-5", "content": content},
    }


def use(tool_id: str, name: str, tool_input: dict) -> dict:
    return {"type": "tool_use", "id": tool_id, "name": name, "input": tool_input}


def result(seconds: float, tool_id: str) -> dict:
    return {
        "type": "user",
        "timestamp": at(seconds),
        "message": {
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": tool_id, "content": "x"}],
        },
    }


def notification(seconds: float) -> dict:
    return {
        "type": "user",
        "timestamp": at(seconds),
        "message": {"role": "user", "content": "<task-notification>done</task-notification>"},
    }


def known_run() -> list[dict]:
    """A run whose every gap is accounted for, so the parts must add up to the span.

    0-5 model; 5-65 cargo test; 65-68 model; 68-69 read; 69-72 model; 72-192 polling loop;
    192-195 model, then the agent ends its turn; 195-615 waiting; 615-620 model; 620-622 git;
    622-625 model, then the final answer.
    """
    return [
        prompt(0),
        turn(5, use("t1", "Bash", {"command": "cargo test -p workgraph"})),
        result(65, "t1"),
        turn(68, use("r1", "Read", {"file_path": "src/lib.rs"})),
        result(69, "r1"),
        turn(72, use("p1", "Bash", {"command": "until grep -q done out.txt; do sleep 5; done"})),
        result(192, "p1"),
        turn(195, text="waiting for the build"),
        notification(615),
        turn(620, use("g1", "Bash", {"command": "git status --short"})),
        result(622, "g1"),
        turn(625, text="all done"),
    ]


def test_classify_matches_commands_and_tool_names():
    assert classify("Bash", {"command": "cargo test -p workgraph"}) == "cargo test"
    assert classify("Bash", {"command": "cargo clippy --all"}) == "cargo build/check"
    assert classify("PowerShell", {"command": "Start-Sleep -Seconds 30"}) == "polling loop"
    assert classify("Bash", {"command": "while ! test -f x; do sleep 2; done"}) == "polling loop"
    assert classify("Bash", {"command": "sleep 60"}) == "polling loop"
    assert classify("Bash", {"command": "cd C:/x && git log -3"}) == "git"
    assert classify("Bash", {"command": "rho.exe build --release"}) == "rho build/run"
    assert classify("Bash", {"command": "target/debug/tool.exe --check"}) == "program run"
    assert classify("Bash", {"command": "rg TODO src"}) == "search/read"
    assert classify("Bash", {"command": "python scripts/gen.py"}) == "script"
    assert classify("Bash", {"command": "make everything"}) == "other shell"
    assert classify("Read", {"file_path": "a"}) == "search/read"
    assert classify("Grep", {"pattern": "x"}) == "search/read"
    assert classify("Edit", {"file_path": "a"}) == "edit"
    assert classify("Agent", {"prompt": "x"}) == "spawn subagent"
    assert classify("WebSearch", {"query": "x"}) == "other tool"


def test_known_run_splits_exactly_into_model_waiting_and_tool_classes():
    time = breakdown(known_run())

    assert time["span_seconds"] == 625
    assert time["classes"]["cargo test"] == {"seconds": 60, "calls": 1}
    assert time["classes"]["search/read"] == {"seconds": 1, "calls": 1}
    assert time["classes"]["polling loop"] == {"seconds": 120, "calls": 1}
    assert time["classes"]["git"] == {"seconds": 2, "calls": 1}
    assert time["tool_seconds"] == 183
    assert time["model_seconds"] == 22
    assert time["waiting_seconds"] == 420
    assert time["tool_seconds"] + time["model_seconds"] + time["waiting_seconds"] == time["span_seconds"]


def test_slowest_calls_name_the_call_and_its_start():
    time = breakdown(known_run())

    assert [call["class"] for call in time["slowest"]] == ["polling loop", "cargo test", "git", "search/read"]
    polling, cargo = time["slowest"][0], time["slowest"][1]
    assert polling["seconds"] == 120
    assert polling["at"] == (BASE + timedelta(seconds=72)).isoformat()
    assert polling["command"].startswith("until grep -q done")
    assert cargo["command"] == "cargo test -p workgraph"


def test_split_message_lines_count_a_tool_use_once_from_its_first_line():
    records = [
        prompt(0),
        turn(4, use("t1", "Bash", {"command": "cargo build"}), message_id="m1"),
        # The same message again, streamed as a later line: the call still started at 4 s.
        turn(5, use("t1", "Bash", {"command": "cargo build"}), message_id="m1"),
        result(14, "t1"),
    ]
    time = breakdown(records)

    assert time["classes"]["cargo build/check"] == {"seconds": 10, "calls": 1}
    assert time["model_seconds"] == 4


def test_background_launch_is_its_own_class_and_synthetic_or_bad_records_are_ignored():
    records = [
        prompt(0),
        turn(2, use("b1", "Bash", {"command": "cargo test", "run_in_background": True})),
        result(2, "b1"),
        {"type": "assistant", "timestamp": at(3), "message": {"model": "<synthetic>", "content": []}},
        "not a record",
        {"type": "user", "timestamp": "not a time", "message": {"content": "junk"}},
    ]
    time = breakdown(records)

    assert time["classes"] == {"cargo test (launch)": {"seconds": 0, "calls": 1}}
    assert time["waiting_seconds"] == 0


def test_slowest_is_capped_and_long_commands_are_truncated():
    count = SLOWEST_CALLS + 3
    records = [prompt(0)]
    for index in range(count):
        start = 10 * index + 1
        command = "rg " + "x" * 400 if index == count - 1 else f"rg pattern{index}"
        records.append(turn(start, use(f"s{index}", "Bash", {"command": command})))
        records.append(result(start + index + 1, f"s{index}"))  # call i lasts i + 1 seconds
    time = breakdown(records)

    assert len(time["slowest"]) == SLOWEST_CALLS
    assert [call["seconds"] for call in time["slowest"]] == [float(count - n) for n in range(SLOWEST_CALLS)]
    longest = time["slowest"][0]
    assert longest["seconds"] == count
    assert len(longest["command"]) <= 160


def test_parse_transcript_stores_the_breakdown_in_telemetry(tmp_path):
    path = tmp_path / "agent-a1.jsonl"
    path.write_text("\n".join(json.dumps(record) for record in known_run()) + "\n", encoding="utf-8")

    stats = parse_transcript(path)

    assert stats is not None and stats.time is not None
    assert stats.telemetry()["time"]["waiting_seconds"] == 420
    assert stats.telemetry()["time"]["classes"]["polling loop"]["seconds"] == 120
