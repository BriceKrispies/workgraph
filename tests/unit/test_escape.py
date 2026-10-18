from __future__ import annotations

from workgraph.experiments.escape import find_escapes, normalize_path_text
from workgraph.integrations.claude_code.transcript import ToolCall

ROOT = r"C:\dev\rho"


def call(tool_input, *, name="Read", tool_use_id="t1") -> ToolCall:
    return ToolCall(name=name, input=tool_input, tool_use_id=tool_use_id)


def test_backslashes_become_slashes_and_text_is_lowercased():
    assert normalize_path_text(r"C:\Dev\Rho\src") == "c:/dev/rho/src"


def test_msys_drive_prefix_becomes_a_windows_drive():
    assert normalize_path_text("cd /c/dev/rho && git log") == "cd c:/dev/rho && git log"
    assert normalize_path_text("/D/work") == "d:/work"


def test_repeated_slashes_collapse_before_the_drive_prefix_is_read():
    assert normalize_path_text("c://dev///rho") == "c:/dev/rho"
    assert normalize_path_text("//c/dev") == "c:/dev"


def test_relative_segments_that_look_like_drives_are_left_alone():
    assert normalize_path_text("c:/c/dev") == "c:/c/dev"
    assert normalize_path_text("src/a/b") == "src/a/b"


def test_a_sibling_directory_with_the_root_as_prefix_does_not_match():
    assert find_escapes([call({"file_path": "c:/dev/rho-other/x.rs"})], [ROOT]) == []


def test_the_root_itself_and_paths_under_it_match():
    for value in ["c:/dev/rho", "c:/dev/rho/x", '"c:/dev/rho"', "cd /c/dev/rho && git log"]:
        escapes = find_escapes([call({"command": value})], [ROOT])
        assert len(escapes) == 1, value
        assert escapes[0].root == ROOT
        assert escapes[0].excerpt == value


def test_root_followed_by_shell_operators_matches():
    for value in ["c:/dev/rho;ls", "c:/dev/rho&&ls", "c:/dev/rho|cat", "c:/dev/rho\nls"]:
        assert len(find_escapes([call({"command": value})], [ROOT])) == 1, value


def test_nested_dict_and_list_values_are_searched():
    tool_input = {
        "edits": [{"old": "a", "new": "b"}, {"path": ["x", {"deep": r"C:\dev\rho\pkg\mod.py"}]}],
        "meta": {"description": "unrelated"},
    }
    escapes = find_escapes([call(tool_input, name="Edit")], [ROOT])
    assert [(e.tool, e.root) for e in escapes] == [("Edit", ROOT)]
    assert escapes[0].excerpt == r"C:\dev\rho\pkg\mod.py"


def test_keys_and_non_string_values_are_ignored():
    assert find_escapes([call({"c:/dev/rho": 1, "n": 2, "flag": True})], [ROOT]) == []


def test_one_escape_per_call_and_root():
    tool_input = {"a": "c:/dev/rho/x", "b": "c:/dev/rho/y", "c": "C:/dev/rho/other"}
    escapes = find_escapes([call(tool_input)], [ROOT, "c:/dev/rho-other-clone/../rho"])
    assert [e.root for e in escapes] == [ROOT]

    nested = find_escapes([call(tool_input, tool_use_id="t9")], [r"C:\dev\rho", r"C:\dev\rho\other"])
    assert sorted((e.tool_use_id, e.root) for e in nested) == [
        ("t9", r"C:\dev\rho"),
        ("t9", r"C:\dev\rho\other"),
    ]


def test_each_call_is_checked_separately():
    calls = [call({"f": "c:/dev/rho/a"}, tool_use_id="t1"), call({"f": "c:/other/a"}, tool_use_id="t2")]
    escapes = find_escapes(calls, [ROOT])
    assert [e.tool_use_id for e in escapes] == ["t1"]


def test_excerpt_is_cut_to_300_characters():
    long_value = "c:/dev/rho/" + "x" * 500
    escapes = find_escapes([call({"command": long_value})], [ROOT])
    assert len(escapes[0].excerpt) == 300


def test_empty_roots_are_ignored():
    assert find_escapes([call({"command": "anything"})], ["", "/"]) == []
