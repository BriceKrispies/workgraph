//! The pure half of the `git` command group (IF-12): sync ordering and reconciliation, explicit
//! link writes, and the shape of the changes reply.

use rpds::VectorSync;
use wg_git_feature::spec::{ingest, links, reads};
use wg_json::Value;
use wg_model::entity::git::{Commit, CommitTaskLink};
use wg_model::{Row, Write};
use wg_types::{Id, Timestamp, parse_id};

const REPOSITORY: &str = "018f0000-0000-7000-8000-000000000001";

fn id(text: &str) -> Id {
    parse_id(text).unwrap_or_else(|err| panic!("a valid uuid: {err:?}"))
}

fn now() -> Timestamp {
    Timestamp::parse_iso("2026-10-20T10:00:00+00:00")
        .unwrap_or_else(|err| panic!("a valid timestamp: {err:?}"))
}

fn commit(commit_id: &str, sha: &str) -> Commit {
    Commit {
        id: id(commit_id),
        repository_id: id(REPOSITORY),
        sha: sha.to_owned(),
        tree_sha: None,
        parents: VectorSync::new_sync(),
        author_name: "Dev".to_owned(),
        author_email: "dev@example.com".to_owned(),
        authored_at: None,
        committer_name: "Dev".to_owned(),
        committer_email: "dev@example.com".to_owned(),
        committed_at: None,
        subject: "subject".to_owned(),
        message: "subject".to_owned(),
        trailers: VectorSync::new_sync(),
        is_merge: false,
        patch_id: None,
        insertions: 0,
        deletions: 0,
        files_changed: 0,
        reachable: true,
        unreachable_since: None,
        object_available: true,
        structure_analyzed: false,
        branches: VectorSync::new_sync(),
        ingested_at: now(),
    }
}

fn link(link_id: &str, commit_id: &str, task_id: &str, source: &str) -> CommitTaskLink {
    CommitTaskLink {
        id: id(link_id),
        commit_id: id(commit_id),
        task_id: id(task_id),
        source: source.to_owned(),
        confidence: wg_model::entity::Real(1.0),
        evidence: serde_json::Map::new(),
        attempt_id: None,
        created_by: "human:test".to_owned(),
        created_at: now(),
    }
}

// covers: C095
#[test]
fn new_commits_are_the_unknown_candidates_oldest_first() {
    let candidates = vec!["c3".to_owned(), "c2".to_owned(), "c1".to_owned()];
    let known: std::collections::BTreeSet<String> = ["c1".to_owned()].into_iter().collect();
    assert_eq!(
        ingest::new_shas(&candidates, &known),
        vec!["c2".to_owned(), "c3".to_owned()]
    );
}

// covers: C095
#[test]
fn only_full_syncs_and_non_fast_forward_refs_reconcile_reachability() {
    assert!(ingest::needs_reconcile(true, false));
    assert!(ingest::needs_reconcile(false, true));
    assert!(!ingest::needs_reconcile(false, false));
}

// covers: C095
#[test]
fn rows_written_by_one_decision_keep_their_write_order_in_time() {
    let first = ingest::offset_time(now(), 0);
    let second = ingest::offset_time(now(), 1);
    assert!(first < second, "the later row is stamped later");
    assert_eq!(first, now());
}

// covers: C091
#[test]
fn explicit_link_skips_an_existing_link_and_ties_the_repository_once() {
    let target = commit("018f0000-0000-7000-8000-000000000010", "abc123");
    let task = "018f0000-0000-7000-8000-000000000020";
    let existing = vec![link(
        "018f0000-0000-7000-8000-000000000030",
        "018f0000-0000-7000-8000-000000000010",
        task,
        links::EXPLICIT,
    )];
    let writes = links::explicit_link_writes(
        &target,
        id(task),
        id(REPOSITORY),
        "read_write",
        &existing,
        links::ExplicitLinkIds {
            id: id("018f0000-0000-7000-8000-000000000040"),
            note: "manual",
            actor: "human:test",
            attempt_id: None,
            task_repository_exists: false,
            now: now(),
        },
    );
    assert_eq!(writes.len(), 1, "only the task-repository row is written");
    assert!(matches!(
        writes.first(),
        Some(Write::Insert(Row::TaskRepository(_)))
    ));
}

// covers: C087
#[test]
fn changes_of_a_task_without_commits_have_the_service_key_order() {
    let input = reads::ChangesInput {
        task_id: id("018f0000-0000-7000-8000-000000000050"),
        task_key: "WG-1",
        min_confidence: 0.5,
        commits: &[],
        files: &[],
        analyses: &[],
        surfaces: &[],
    };
    let Value::Dict(entries) = reads::task_changes_value(&input) else {
        panic!("the changes reply is a dict");
    };
    let keys: Vec<String> = entries
        .iter()
        .map(|(key, _)| {
            key.code_points()
                .iter()
                .filter_map(|point| char::from_u32(*point))
                .collect()
        })
        .collect();
    assert_eq!(
        keys,
        [
            "task_id",
            "task_key",
            "min_confidence",
            "commits",
            "cumulative_churn",
            "final_change_surface",
            "definitions",
        ]
    );
}
