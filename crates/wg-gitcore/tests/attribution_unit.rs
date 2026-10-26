//! Ports of `tests/unit/test_attribution.py` (message-inference rules), one test per Python test.

use wg_gitcore::{Candidate, Evidence, Trailer, message_candidates};

fn keys(found: &[Candidate]) -> Vec<&str> {
    found.iter().map(|c| c.task_ref.as_str()).collect()
}

fn ambiguous(candidate: &Candidate) -> bool {
    match &candidate.evidence {
        Evidence::Message { ambiguous, .. } => *ambiguous,
        Evidence::Trailer { .. } => false,
    }
}

fn task_id(value: &str) -> Vec<Trailer> {
    vec![Trailer {
        key: "Task-ID".to_owned(),
        value: value.to_owned(),
    }]
}

#[test]
fn test_empty_prefix_list_matches_nothing() {
    assert!(message_candidates("fixes WG-1", &[], &[]).is_empty());
}

#[test]
fn test_single_prefix_case_insensitive_and_normalised() {
    let found = message_candidates("fix wg-12 and WG-3", &["WG"], &[]);
    assert_eq!(keys(&found), ["WG-12", "WG-3"]);
    assert!(found.iter().all(|c| c.confidence == 0.35));
    assert!(ambiguous(&found[0]));
}

#[test]
fn test_single_key_has_full_confidence() {
    let found = message_candidates("fix wg-12", &["WG"], &[]);
    assert_eq!(keys(&found), ["WG-12"]);
    assert_eq!(found[0].confidence, 0.5);
    assert!(!ambiguous(&found[0]));
}

#[test]
fn test_two_prefixes_both_recognised() {
    let found = message_candidates("closes ABC-7, relates to WG-2", &["ABC", "WG"], &[]);
    assert_eq!(keys(&found), ["ABC-7", "WG-2"]);
    assert!(found.iter().all(|c| c.confidence == 0.35));
    assert!(ambiguous(&found[0]));
}

#[test]
fn test_mixed_case_prefix_is_upper_cased() {
    assert_eq!(
        keys(&message_candidates("see abc-7", &["Abc"], &[])),
        ["ABC-7"]
    );
}

#[test]
fn test_longer_prefix_does_not_yield_shorter_match() {
    let found = message_candidates("ABCD-12 only", &["ABC", "ABCD"], &[]);
    assert_eq!(keys(&found), ["ABCD-12"]);
    assert!(!ambiguous(&found[0]));
}

#[test]
fn test_shorter_prefix_inside_longer_key_is_not_matched() {
    assert!(message_candidates("XABC-1 only", &["ABC"], &[]).is_empty());
    assert_eq!(
        keys(&message_candidates("XABC-1 then ABC-2", &["ABC"], &[])),
        ["ABC-2"]
    );
}

#[test]
fn test_lookahead_semantics_follow_the_original_rule() {
    // the number is greedy, so a whole run of digits is one key; a following letter is allowed
    assert_eq!(
        keys(&message_candidates("ABC-12345", &["ABC"], &[])),
        ["ABC-12345"]
    );
    assert_eq!(
        keys(&message_candidates("ABC-12a", &["ABC"], &[])),
        ["ABC-12"]
    );
}

#[test]
fn test_duplicates_are_collapsed_in_order_of_appearance() {
    let found = message_candidates("ABC-5 then WG-1 then abc-5", &["WG", "ABC"], &[]);
    assert_eq!(keys(&found), ["ABC-5", "WG-1"]);
    match &found[0].evidence {
        Evidence::Message { all_matches, .. } => assert_eq!(all_matches, &["ABC-5", "WG-1"]),
        Evidence::Trailer { .. } => panic!("message candidate expected"),
    }
}

#[test]
fn test_trailer_lines_are_excluded_from_prose() {
    let trailers = task_id("ABC-7");
    let message = "implement\n\nTask-ID: ABC-7";
    assert!(message_candidates(message, &["ABC"], &trailers).is_empty());
    let with_prose = format!("{message}\nalso ABC-8");
    assert_eq!(
        keys(&message_candidates(&with_prose, &["ABC"], &trailers)),
        ["ABC-8"]
    );
}
