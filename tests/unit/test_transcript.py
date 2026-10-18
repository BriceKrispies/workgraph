from __future__ import annotations

import json

from workgraph.integrations.claude_code.transcript import (
    ToolCall,
    TranscriptStats,
    parse_transcript,
    tool_calls,
)


def parsed(path) -> TranscriptStats:
    stats = parse_transcript(path)
    assert stats is not None
    return stats


def assistant(
    message_id,
    output,
    *,
    model="claude-haiku-4-5-20251001",
    content=None,
    timestamp="2026-10-14T08:54:31.594Z",
):
    return {
        "type": "assistant",
        "timestamp": timestamp,
        "message": {
            "id": message_id,
            "model": model,
            "content": content or [{"type": "text", "text": "ok"}],
            "usage": {
                "input_tokens": 3,
                "cache_creation_input_tokens": 100,
                "cache_read_input_tokens": 1000,
                "output_tokens": output,
            },
        },
    }


def tool_use(tool_id, name):
    return {"type": "tool_use", "id": tool_id, "name": name, "input": {}}


def prompt(text, timestamp="2026-10-14T08:54:30.000Z"):
    return {
        "type": "user",
        "version": "2.1.289",
        "timestamp": timestamp,
        "message": {"role": "user", "content": text},
    }


def write_lines(path, records):
    path.write_text(
        "\n".join(r if isinstance(r, str) else json.dumps(r) for r in records) + "\n", encoding="utf-8"
    )
    return path


def test_split_message_lines_are_not_double_counted(tmp_path):
    transcript = write_lines(
        tmp_path / "agent-a.jsonl",
        [
            prompt("Do the thing"),
            assistant("msg_1", 3, content=[{"type": "text", "text": "a"}]),
            assistant("msg_1", 3, content=[tool_use("t1", "Bash")]),
            assistant("msg_1", 365, content=[tool_use("t2", "Read")]),
            assistant("msg_2", 10),
        ],
    )
    stats = parsed(transcript)
    assert stats is not None
    assert stats.turns == 2
    assert stats.usage["output_tokens"] == 375  # 365 for msg_1 (max line) + 10 for msg_2
    assert stats.usage["input_tokens"] == 6
    assert stats.usage["cache_creation_input_tokens"] == 200
    assert stats.usage["cache_read_input_tokens"] == 2000
    # tool_use blocks are collected from every line of the split message
    assert stats.tool_uses == {"Bash": 1, "Read": 1}
    assert stats.models == {"claude-haiku-4-5-20251001": 2}


def test_equal_output_tokens_prefer_the_last_line(tmp_path):
    transcript = write_lines(
        tmp_path / "agent-tie.jsonl",
        [assistant("msg_1", 7, model="first"), assistant("msg_1", 7, model="second")],
    )
    stats = parsed(transcript)
    assert stats.models == {"second": 1}
    assert stats.model == "second"


def test_synthetic_messages_are_ignored(tmp_path):
    transcript = write_lines(
        tmp_path / "agent-s.jsonl",
        [
            prompt("hi"),
            assistant("msg_1", 50),
            assistant("msg_x", 999, model="<synthetic>", content=[tool_use("t9", "Write")]),
        ],
    )
    stats = parsed(transcript)
    assert stats.models == {"claude-haiku-4-5-20251001": 1}
    assert stats.turns == 1
    assert stats.usage["output_tokens"] == 50
    assert stats.tool_uses == {}


def test_tool_uses_counted_once_per_id_and_errors_counted(tmp_path):
    error_result = {
        "type": "user",
        "message": {
            "role": "user",
            "content": [
                {"type": "tool_result", "tool_use_id": "t1", "is_error": True, "content": "boom"},
                {"type": "tool_result", "tool_use_id": "t2", "is_error": False, "content": "fine"},
            ],
        },
    }
    transcript = write_lines(
        tmp_path / "agent-t.jsonl",
        [
            prompt("go"),
            assistant("msg_1", 5, content=[tool_use("t1", "Bash"), tool_use("t2", "Edit")]),
            assistant("msg_1", 9, content=[tool_use("t1", "Bash"), tool_use("t2", "Edit")]),
            assistant("msg_2", 9, content=[tool_use("t3", "Bash")]),
            error_result,
        ],
    )
    stats = parsed(transcript)
    assert stats.tool_uses == {"Bash": 2, "Edit": 1}
    assert stats.tool_errors == 1


def test_markers_and_prompt_length_come_from_the_first_prompt(tmp_path):
    text = "Task-ID: WG-1118\nTrial-ID: 0b9f6c2e-1111-4a2b-9c3d-123456789abc\nAlso mention WG-9 inline."
    transcript = write_lines(
        tmp_path / "agent-m.jsonl",
        [
            prompt(text),
            prompt("Task-ID: WG-7 in a later prompt"),
            assistant("msg_1", 1),
        ],
    )
    stats = parsed(transcript)
    assert stats.task_keys == ("WG-1118",)
    assert stats.trial_ids == ("0b9f6c2e-1111-4a2b-9c3d-123456789abc",)
    assert stats.prompt_chars == len(text)


def test_list_prompt_content_is_joined_text(tmp_path):
    transcript = write_lines(
        tmp_path / "agent-l.jsonl",
        [
            {
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{"type": "text", "text": "Task-ID: WG-2"}, {"type": "image", "source": {}}],
                },
            }
        ],
    )
    stats = parsed(transcript)
    assert stats.task_keys == ("WG-2",)
    assert stats.turns == 0
    assert stats.model is None


def test_sidecar_is_read_when_present(tmp_path):
    transcript = write_lines(tmp_path / "agent-abc.jsonl", [prompt("x"), assistant("msg_1", 1)])
    (tmp_path / "agent-abc.meta.json").write_text(
        json.dumps({"agentType": "general-purpose", "description": "Art r3: sky", "model": "haiku"}),
        encoding="utf-8",
    )
    stats = parsed(transcript)
    assert stats.requested_model == "haiku"
    assert stats.agent_type == "general-purpose"
    assert stats.description == "Art r3: sky"
    assert stats.model == "claude-haiku-4-5-20251001"


def test_missing_sidecar_leaves_sidecar_fields_empty(tmp_path):
    transcript = write_lines(tmp_path / "agent-nometa.jsonl", [prompt("x"), assistant("msg_1", 1)])
    stats = parsed(transcript)
    assert stats is not None
    assert stats.requested_model is None and stats.agent_type is None and stats.description is None


def test_missing_file_returns_none(tmp_path):
    assert parse_transcript(tmp_path / "agent-gone.jsonl") is None


def test_malformed_lines_are_skipped(tmp_path):
    transcript = write_lines(
        tmp_path / "agent-bad.jsonl",
        [
            prompt("still here"),
            "{not json",
            "[1, 2, 3]",
            "",
            assistant("msg_1", 4),
            '{"type": "attachment", "timestamp": "2026-10-14T08:55:00.000Z"}',
        ],
    )
    stats = parsed(transcript)
    assert stats.prompt_chars == len("still here")
    assert stats.turns == 1
    assert stats.usage["output_tokens"] == 4
    assert stats.started_at == "2026-10-14T08:54:30.000Z"
    assert stats.ended_at == "2026-10-14T08:55:00.000Z"


def test_telemetry_is_json_safe(tmp_path):
    transcript = write_lines(
        tmp_path / "agent-j.jsonl",
        [prompt("Task-ID: WG-3"), assistant("msg_1", 2, content=[tool_use("t1", "Grep")])],
    )
    stats = parsed(transcript)
    data = stats.telemetry()
    assert json.loads(json.dumps(data)) == data
    assert data["task_keys"] == ["WG-3"]
    assert data["tool_uses"] == {"Grep": 1}
    assert "usage" not in data


def test_tool_calls_dedupe_repeated_blocks_and_return_inputs(tmp_path):
    read = {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "c:/dev/rho/pkg/mod.py"}}
    bash = {"type": "tool_use", "id": "t2", "name": "Bash", "input": {"command": "git log"}}
    transcript = write_lines(
        tmp_path / "agent-calls.jsonl",
        [
            prompt("go"),
            assistant("msg_1", 5, content=[read]),
            assistant("msg_1", 9, content=[read, bash]),
            assistant("msg_2", 9, content=[{"type": "text", "text": "no tools here"}]),
            assistant("msg_3", 4, model="<synthetic>", content=[tool_use("t9", "Edit")]),
        ],
    )
    calls = tool_calls(transcript)
    assert calls == [
        ToolCall(name="Read", input={"file_path": "c:/dev/rho/pkg/mod.py"}, tool_use_id="t1"),
        ToolCall(name="Bash", input={"command": "git log"}, tool_use_id="t2"),
    ]


def test_tool_calls_missing_file_is_empty(tmp_path):
    assert tool_calls(tmp_path / "absent.jsonl") == []
