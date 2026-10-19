"""Message-inference rules for commit attribution."""

from __future__ import annotations

from workgraph.integrations.git.attribution import message_candidates


def keys(found):
    return [c.task_ref for c in found]


def test_empty_prefix_list_matches_nothing():
    assert message_candidates("fixes WG-1", [], []) == []


def test_single_prefix_case_insensitive_and_normalised():
    found = message_candidates("fix wg-12 and WG-3", ["WG"], [])
    assert keys(found) == ["WG-12", "WG-3"]
    assert all(c.source == "message_inference" and c.confidence == 0.35 for c in found)
    assert found[0].evidence["ambiguous"] is True


def test_single_key_has_full_confidence():
    found = message_candidates("fix wg-12", ["WG"], [])
    assert keys(found) == ["WG-12"]
    assert found[0].confidence == 0.5
    assert found[0].evidence["ambiguous"] is False


def test_two_prefixes_both_recognised():
    found = message_candidates("closes ABC-7, relates to WG-2", ["ABC", "WG"], [])
    assert keys(found) == ["ABC-7", "WG-2"]
    assert all(c.confidence == 0.35 for c in found)
    assert found[0].evidence["ambiguous"] is True


def test_mixed_case_prefix_is_upper_cased():
    assert keys(message_candidates("see abc-7", ["Abc"], [])) == ["ABC-7"]


def test_longer_prefix_does_not_yield_shorter_match():
    found = message_candidates("ABCD-12 only", ["ABC", "ABCD"], [])
    assert keys(found) == ["ABCD-12"]
    assert found[0].evidence["ambiguous"] is False


def test_shorter_prefix_inside_longer_key_is_not_matched():
    assert message_candidates("XABC-1 only", ["ABC"], []) == []
    assert keys(message_candidates("XABC-1 then ABC-2", ["ABC"], [])) == ["ABC-2"]


def test_lookahead_semantics_follow_the_original_rule():
    # the number is greedy, so a whole run of digits is one key; a following letter is allowed
    assert keys(message_candidates("ABC-12345", ["ABC"], [])) == ["ABC-12345"]
    assert keys(message_candidates("ABC-12a", ["ABC"], [])) == ["ABC-12"]


def test_duplicates_are_collapsed_in_order_of_appearance():
    found = message_candidates("ABC-5 then WG-1 then abc-5", ["WG", "ABC"], [])
    assert keys(found) == ["ABC-5", "WG-1"]
    assert found[0].evidence["all_matches"] == ["ABC-5", "WG-1"]


def test_trailer_lines_are_excluded_from_prose():
    trailers = [{"key": "Task-ID", "value": "ABC-7"}]
    message = "implement\n\nTask-ID: ABC-7"
    assert message_candidates(message, ["ABC"], trailers) == []
    assert keys(message_candidates(message + "\nalso ABC-8", ["ABC"], trailers)) == ["ABC-8"]
