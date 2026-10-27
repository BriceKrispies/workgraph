//! Git-family reads (SH-09): commit lookups by SHA and prefix, the repository commit list and its
//! paging and filters, the joined task-link reads, the churn inputs (file changes, path totals,
//! symbols, analyses), equivalences and their candidates, object gaps, reachability and the
//! change surfaces a task leaves.
//!
//! The database is built from `contracts/schema-0011.sql` (tables, the FTS virtual table and the
//! triggers), as `sh04_tasks.rs` builds it.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_git_feature::service::store;
use wg_model::entity::{
    Commit, CommitAnalysis, CommitEquivalence, CommitFileChange, CommitSymbol, CommitTaskLink,
    GitObjectGap, Real, Repository, Task, TaskChangeSurface, Workspace,
};
use wg_model::{JsonObject, Row, Write, derive_id};
use wg_store::{Location, Store};
use wg_types::{AccessMode, Id, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError};

type TestResult = Result<(), Box<dyn Error>>;

/// Unwraps a fixture value that is valid by construction.
fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn schema() -> Result<String, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/schema-0011.sql");
    let text = std::fs::read_to_string(path)?;
    let wanted: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|block| {
            (block.starts_with("CREATE TABLE") && !block.starts_with("CREATE TABLE 'task_fts_"))
                || block.starts_with("CREATE VIRTUAL TABLE")
                || block.starts_with("CREATE TRIGGER")
        })
        .collect();
    Ok(wanted.join(";\n"))
}

fn store() -> Result<Store, Box<dyn Error>> {
    let mut store = Store::open(&Location::Memory)?;
    let ddl = schema()?;
    store.write(|session| session.execute_batch(&ddl))?;
    Ok(store)
}

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn id(n: u32) -> Id {
    derive_id(&entropy(9), n)
}

/// The instant `second` seconds into the fixture hour, so the fixture can order past a minute.
fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(
        2026,
        10,
        25,
        12,
        second / 60,
        second % 60,
        0,
    ))
}

fn obj(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

fn put(store: &mut Store, row: impl Into<Row>) -> TestResult {
    store.write(|session| session.apply_write(&Write::insert(row)))?;
    Ok(())
}

fn workspace() -> Workspace {
    Workspace {
        id: id(0),
        slug: "default".into(),
        name: "Default".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 10,
        settings: obj(json!({})),
        archived_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

fn repository(n: u32, name: &str) -> Repository {
    Repository {
        id: id(n),
        workspace_id: id(0),
        name: name.into(),
        description: String::new(),
        identity_fingerprint: None,
        default_branch: Some("main".into()),
        tracked_branches: VectorSync::new_sync(),
        access_mode: AccessMode::ReadWrite,
        health_status: "ok".into(),
        health_detail: Default::default(),
        health_checked_at: None,
        is_shallow: false,
        last_scanned_at: None,
        archived_at: None,
        metadata: Default::default(),
        created_at: at(2),
        updated_at: at(2),
    }
}

fn task(n: u32, number: i64) -> Task {
    Task {
        id: id(n),
        workspace_id: id(0),
        number,
        key: ok(TaskKey::parse(&format!("WG-{number}"))),
        title: format!("task {number}"),
        description: "plain description".into(),
        description_hash: "h".into(),
        acceptance_criteria: VectorSync::new_sync(),
        status: TaskStatus::Backlog,
        priority: TaskPriority::Medium,
        category: None,
        parent_id: None,
        assignee: None,
        assigned_agent: None,
        claimed_by: None,
        claim_expires_at: None,
        auto_execute: false,
        estimated_effort: None,
        metadata: obj(json!({})),
        due_at: None,
        scheduled_at: None,
        created_by: "human:brice".into(),
        version: 1,
        created_at: at(1),
        updated_at: at(1),
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    }
}

/// A commit of `repository` n with the given SHA, dates (seconds into the fixture day), patch id,
/// reachability and author facts.
#[allow(clippy::too_many_arguments)]
fn commit(
    n: u32,
    repository: u32,
    sha: &str,
    committed: Option<u8>,
    patch: Option<&str>,
    reachable: bool,
    subject: &str,
    authored: Option<u8>,
) -> Commit {
    Commit {
        id: id(n),
        repository_id: id(repository),
        sha: sha.into(),
        tree_sha: None,
        parents: VectorSync::new_sync(),
        author_name: "Ada".into(),
        author_email: "ada@example.test".into(),
        authored_at: authored.map(at),
        committer_name: "Ada".into(),
        committer_email: "ada@example.test".into(),
        committed_at: committed.map(at),
        subject: subject.into(),
        message: format!("{subject}\n"),
        trailers: VectorSync::new_sync(),
        is_merge: false,
        patch_id: patch.map(str::to_owned),
        insertions: 5,
        deletions: 1,
        files_changed: 2,
        reachable,
        unreachable_since: None,
        object_available: true,
        structure_analyzed: true,
        branches: VectorSync::new_sync(),
        ingested_at: at(3),
    }
}

fn link(
    n: u32,
    commit_n: u32,
    task_n: u32,
    source: &str,
    confidence: f64,
    second: u8,
) -> CommitTaskLink {
    CommitTaskLink {
        id: id(n),
        commit_id: id(commit_n),
        task_id: id(task_n),
        source: source.into(),
        confidence: Real(confidence),
        evidence: obj(json!({})),
        attempt_id: None,
        created_by: "human:brice".into(),
        created_at: at(second),
    }
}

fn file_change(
    n: u32,
    commit_n: u32,
    path: &str,
    insertions: i64,
    deletions: i64,
) -> CommitFileChange {
    CommitFileChange {
        id: id(n),
        commit_id: id(commit_n),
        path: path.into(),
        old_path: None,
        change_type: "M".into(),
        similarity: None,
        insertions,
        deletions,
        is_binary: false,
        language: Some("rust".into()),
        file_class: "source".into(),
        module: "core".into(),
    }
}

/// Sorts a list of ids, so an expected list can be written in any order.
fn sorted(mut ids: Vec<Id>) -> Vec<Id> {
    ids.sort();
    ids
}

/// The ids of `items`, sorted, so tests can compare sets without depending on hash-derived order.
fn sorted_ids<T>(items: &[T], key: impl Fn(&T) -> Id) -> Vec<Id> {
    let mut ids: Vec<Id> = items.iter().map(key).collect();
    ids.sort();
    ids
}

/// The fixture: workspace, two repositories, two tasks, six commits in repository 10 and one in
/// repository 11, task links, file changes, symbols, an analysis, equivalences, a gap and two
/// change surfaces.
fn fixture() -> Result<Store, Box<dyn Error>> {
    let mut store = store()?;
    put(&mut store, workspace())?;
    put(&mut store, repository(10, "core"))?;
    put(&mut store, repository(11, "other"))?;
    put(&mut store, task(20, 1))?;
    put(&mut store, task(21, 2))?;

    // Repository 10. Dates: 101 at 10s, 102 at 20s, 103 at 30s, 104 at 40s, 105 and 106 undated.
    put(
        &mut store,
        commit(
            101,
            10,
            "aaaa1111",
            Some(10),
            Some("p1"),
            true,
            "first",
            Some(5),
        ),
    )?;
    put(
        &mut store,
        commit(
            102,
            10,
            "aaab2222",
            Some(20),
            Some("p2"),
            true,
            "second",
            Some(6),
        ),
    )?;
    put(
        &mut store,
        commit(
            103,
            10,
            "bbbb3333",
            Some(30),
            None,
            false,
            "amend me",
            Some(7),
        ),
    )?;
    put(
        &mut store,
        commit(
            104,
            10,
            "cccc4444",
            Some(40),
            Some("p3"),
            true,
            "amended",
            Some(8),
        ),
    )?;
    put(
        &mut store,
        commit(105, 10, "aaaa9999", None, None, true, "undated", None),
    )?;
    put(
        &mut store,
        commit(106, 10, "dddd6666", None, None, false, "amend me", None),
    )?;
    // Repository 11 has a commit with the same SHA prefix as repository 10's first commit.
    put(
        &mut store,
        commit(
            201,
            11,
            "aaaa1111",
            Some(10),
            None,
            true,
            "elsewhere",
            Some(5),
        ),
    )?;

    // Task 20 is linked to 101 twice (explicit and a weaker heuristic) and to 102; task 21 to 103.
    put(&mut store, link(301, 101, 20, "explicit", 1.0, 1))?;
    put(&mut store, link(302, 101, 20, "heuristic", 0.4, 2))?;
    put(&mut store, link(303, 102, 20, "explicit", 0.9, 3))?;
    put(&mut store, link(304, 103, 21, "explicit", 1.0, 4))?;

    // Churn inputs: 101 touches a.rs and b.rs, 102 touches a.rs again.
    put(&mut store, file_change(401, 101, "src/a.rs", 3, 1))?;
    put(&mut store, file_change(402, 101, "src/b.rs", 2, 0))?;
    put(&mut store, file_change(403, 102, "src/a.rs", 4, 4))?;
    for (n, name, kind) in [(501, "run", "function"), (502, "Config", "struct")] {
        put(
            &mut store,
            CommitSymbol {
                id: id(n),
                file_change_id: id(401),
                symbol_kind: kind.into(),
                name: name.into(),
                change_kind: "modified".into(),
                start_line: Some(1),
                end_line: Some(9),
                lines_changed: 3,
            },
        )?;
    }
    put(
        &mut store,
        CommitAnalysis {
            commit_id: id(101),
            analyzer_version: "1".into(),
            files_parsed: 2,
            files_fallback: 0,
            functions_affected: 1,
            types_affected: 1,
            detail: obj(json!({})),
            analyzed_at: at(50),
        },
    )?;

    // 104 rewrites 103; 102 is a cherry-pick of 101.
    for (n, original, equivalent, kind) in
        [(601, 103, 104, "rewrite"), (602, 101, 102, "cherry_pick")]
    {
        put(
            &mut store,
            CommitEquivalence {
                id: id(n),
                repository_id: id(10),
                original_commit_id: id(original),
                equivalent_commit_id: id(equivalent),
                kind: kind.into(),
                evidence: obj(json!({})),
                created_at: at(60),
            },
        )?;
    }

    put(
        &mut store,
        GitObjectGap {
            id: id(701),
            repository_id: id(10),
            sha: "dead0000".into(),
            context: "parent".into(),
            detected_at: at(70),
        },
    )?;

    for (n, repository_n) in [(801, 10), (802, 11)] {
        put(
            &mut store,
            TaskChangeSurface {
                id: id(n),
                task_id: id(20),
                repository_id: id(repository_n),
                attempt_id: None,
                base_sha: "base1".into(),
                final_sha: "final1".into(),
                files_changed: 2,
                insertions: 5,
                deletions: 4,
                renames: 0,
                binary_files: 0,
                functions_affected: 1,
                types_affected: 0,
                detail: obj(json!({})),
                computed_at: at(80),
                updated_at: at(80),
            },
        )?;
    }
    Ok(store)
}

#[test]
fn commits_are_found_by_sha_prefix_and_id() -> TestResult {
    let mut store = fixture()?;

    let exact = store
        .read(|s| store::commit_by_sha(s, id(10), "aaaa1111"))?
        .ok_or("aaaa1111 missing")?;
    assert_eq!(exact.id, id(101));
    // The same SHA in another repository is a different row.
    let elsewhere = store
        .read(|s| store::commit_by_sha(s, id(11), "aaaa1111"))?
        .ok_or("aaaa1111 missing in repository 11")?;
    assert_eq!(elsewhere.id, id(201));
    assert!(
        store
            .read(|s| store::commit_by_sha(s, id(10), "aaaa"))?
            .is_none()
    );

    // A prefix matches every commit it starts, by SHA; a wildcard is an ordinary character.
    let prefixed = store.read(|s| store::commits_by_sha_prefix(s, id(10), "aaaa"))?;
    assert_eq!(
        sorted_ids(&prefixed, |c| c.id),
        sorted(vec![id(101), id(105)])
    );
    assert_eq!(prefixed[0].sha, "aaaa1111");
    assert!(
        store
            .read(|s| store::commits_by_sha_prefix(s, id(10), "a%"))?
            .is_empty()
    );

    let shas = store.read(|s| store::commit_shas(s, id(10)))?;
    assert_eq!(
        shas,
        vec![
            "aaaa1111", "aaaa9999", "aaab2222", "bbbb3333", "cccc4444", "dddd6666"
        ]
    );

    let by_id = store.read(|s| store::commits_by_ids(s, &[id(101), id(103)]))?;
    assert_eq!(sorted_ids(&by_id, |c| c.id), sorted(vec![id(101), id(103)]));
    assert!(store.read(|s| store::commits_by_ids(s, &[]))?.is_empty());
    Ok(())
}

#[test]
fn repository_commit_list_pages_newest_first_with_filters() -> TestResult {
    let mut store = fixture()?;

    let total = store.read(|s| store::repository_commits_count(s, id(10), false, false))?;
    assert_eq!(total, 6);
    // Reachable only leaves out 103 and 106; linked only keeps 101, 102 and 103.
    assert_eq!(
        store.read(|s| store::repository_commits_count(s, id(10), true, false))?,
        4
    );
    assert_eq!(
        store.read(|s| store::repository_commits_count(s, id(10), false, true))?,
        3
    );
    assert_eq!(
        store.read(|s| store::repository_commits_count(s, id(10), true, true))?,
        2
    );

    // Newest first; the undated commits sort last (NULL is smallest, so DESC puts it last).
    let first_two =
        store.read(|s| store::repository_commits_page(s, id(10), false, false, 2, 0))?;
    let shas: Vec<&str> = first_two.iter().map(|c| c.sha.as_str()).collect();
    assert_eq!(shas, vec!["cccc4444", "bbbb3333"]);

    let middle = store.read(|s| store::repository_commits_page(s, id(10), false, false, 2, 2))?;
    let shas: Vec<&str> = middle.iter().map(|c| c.sha.as_str()).collect();
    assert_eq!(shas, vec!["aaab2222", "aaaa1111"]);

    let last = store.read(|s| store::repository_commits_page(s, id(10), false, false, 2, 4))?;
    assert_eq!(last.len(), 2);
    assert_eq!(
        sorted_ids(&last, |c| c.id),
        sorted(vec![id(105), id(106)]),
        "the two undated commits fill the final page"
    );

    let reachable_page =
        store.read(|s| store::repository_commits_page(s, id(10), true, false, 2, 0))?;
    let shas: Vec<&str> = reachable_page.iter().map(|c| c.sha.as_str()).collect();
    assert_eq!(shas, vec!["cccc4444", "aaab2222"]);

    let linked = store.read(|s| store::repository_commits_page(s, id(10), false, true, 10, 0))?;
    assert_eq!(linked.len(), 3);
    Ok(())
}

#[test]
fn task_links_come_back_with_their_task_keys_and_commit_facts() -> TestResult {
    let mut store = fixture()?;

    let linked = store.read(|s| store::links_with_task_keys(s, &[id(101), id(103)]))?;
    assert_eq!(linked.len(), 3);
    let mut keys: Vec<(Id, String)> = linked
        .iter()
        .map(|row| (row.link.commit_id, row.task_key.clone()))
        .collect();
    keys.sort();
    let mut expected = vec![
        (id(101), "WG-1".to_owned()),
        (id(101), "WG-1".to_owned()),
        (id(103), "WG-2".to_owned()),
    ];
    expected.sort();
    assert_eq!(keys, expected);
    assert!(
        store
            .read(|s| store::links_with_task_keys(s, &[id(104)]))?
            .is_empty()
    );

    let all = store.read(|s| store::commit_task_links_between(s, id(101), id(20), None))?;
    assert_eq!(all.len(), 2);
    let heuristic =
        store.read(|s| store::commit_task_links_between(s, id(101), id(20), Some("heuristic")))?;
    assert_eq!(heuristic.len(), 1);
    assert_eq!(heuristic[0].id, id(302));
    assert!(
        store
            .read(|s| store::commit_task_links_between(s, id(101), id(21), None))?
            .is_empty()
    );

    // The task's commits at or above 0.5: 101 (explicit) and 102; the 0.4 heuristic link drops out.
    let joined = store.read(|s| store::task_commit_links(s, id(20), 0.5))?;
    assert_eq!(joined.len(), 2);
    for row in &joined {
        assert_eq!(row.repository_name, "core");
        assert_eq!(row.link.task_id, id(20));
        assert_eq!(row.link.commit_id, row.commit.id);
    }
    let commits = sorted_ids(&joined, |row| row.commit.id);
    assert_eq!(commits, sorted(vec![id(101), id(102)]));

    let everything = store.read(|s| store::task_commit_links(s, id(20), 0.0))?;
    assert_eq!(everything.len(), 3);
    // Ordered by commit date: the two links to 101 come before the one to 102.
    assert_eq!(everything.last().map(|row| row.commit.id), Some(id(102)));
    Ok(())
}

#[test]
fn churn_inputs_are_read_per_commit_and_summed_per_path() -> TestResult {
    let mut store = fixture()?;

    let changes = store.read(|s| store::file_changes(s, &[id(101)]))?;
    let paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, vec!["src/a.rs", "src/b.rs"]);
    assert_eq!(changes[0].language.as_deref(), Some("rust"));

    let totals = store.read(|s| store::path_totals(s, &[id(101), id(102)]))?;
    assert_eq!(totals.len(), 2);
    let a = &totals[0];
    assert_eq!(
        (a.path.as_str(), a.changes, a.insertions, a.deletions),
        ("src/a.rs", 2, 7, 5),
        "a.rs is touched by two commits and sums across them"
    );
    let b = &totals[1];
    assert_eq!(
        (b.path.as_str(), b.changes, b.insertions, b.deletions),
        ("src/b.rs", 1, 2, 0)
    );
    assert!(store.read(|s| store::path_totals(s, &[]))?.is_empty());

    let symbols = store.read(|s| store::commit_symbols(s, &[id(401)]))?;
    assert_eq!(symbols.len(), 2);
    assert!(
        store
            .read(|s| store::commit_symbols(s, &[id(403)]))?
            .is_empty()
    );

    let analyses = store.read(|s| store::commit_analyses(s, &[id(101), id(102)]))?;
    assert_eq!(analyses.len(), 1);
    assert_eq!(analyses[0].commit_id, id(101));
    assert_eq!(analyses[0].functions_affected, 1);
    Ok(())
}

#[test]
fn equivalences_and_their_candidates_are_found_both_ways() -> TestResult {
    let mut store = fixture()?;

    // 103 is the original of the rewrite; 102 is the equivalent of the cherry-pick.
    let of_103 = store.read(|s| store::equivalences_of_commit(s, id(103)))?;
    assert_eq!(of_103.len(), 1);
    assert_eq!(of_103[0].kind, "rewrite");
    let of_102 = store.read(|s| store::equivalences_of_commit(s, id(102)))?;
    assert_eq!(of_102.len(), 1);
    assert_eq!(of_102[0].kind, "cherry_pick");
    assert!(
        store
            .read(|s| store::equivalences_of_commit(s, id(105)))?
            .is_empty()
    );

    let found: CommitEquivalence = store
        .read(|s| store::equivalence(s, id(103), id(104)))?
        .ok_or("rewrite missing")?;
    assert_eq!(found.id, id(601));
    assert!(
        store
            .read(|s| store::equivalence(s, id(104), id(103)))?
            .is_none()
    );

    // Same patch id in repository 10, other than the commit itself.
    let same_patch = store.read(|s| store::commits_with_patch_id(s, id(10), "p1", id(999)))?;
    assert_eq!(sorted_ids(&same_patch, |c| c.id), vec![id(101)]);
    assert!(
        store
            .read(|s| store::commits_with_patch_id(s, id(10), "p1", id(101)))?
            .is_empty()
    );

    // Unreachable commits by author, author date and subject: 103 matches 104's facts, and 106
    // matches an undated author date only with another undated one.
    let by_date = store.read(|s| {
        store::unreachable_commits_matching(
            s,
            id(10),
            "ada@example.test",
            Some(at(7)),
            "amend me",
            id(104),
        )
    })?;
    assert_eq!(sorted_ids(&by_date, |c| c.id), vec![id(103)]);
    let undated = store.read(|s| {
        store::unreachable_commits_matching(
            s,
            id(10),
            "ada@example.test",
            None,
            "amend me",
            id(104),
        )
    })?;
    assert_eq!(sorted_ids(&undated, |c| c.id), vec![id(106)]);
    assert!(
        store
            .read(|s| store::unreachable_commits_matching(
                s,
                id(10),
                "ada@example.test",
                Some(at(7)),
                "amend me",
                id(103)
            ))?
            .is_empty()
    );
    Ok(())
}

#[test]
fn gaps_reachability_and_change_surfaces() -> TestResult {
    let mut store = fixture()?;

    let gap = store
        .read(|s| store::git_object_gap(s, id(10), "dead0000"))?
        .ok_or("gap missing")?;
    assert_eq!(gap.context, "parent");
    assert!(
        store
            .read(|s| store::git_object_gap(s, id(11), "dead0000"))?
            .is_none()
    );
    assert_eq!(store.read(|s| store::git_object_gaps(s, id(10)))?.len(), 1);
    assert!(
        store
            .read(|s| store::git_object_gaps(s, id(11)))?
            .is_empty()
    );

    let reachability = store.read(|s| store::commit_reachability(s, id(10)))?;
    assert_eq!(reachability.len(), 6);
    let unreachable: Vec<&str> = reachability
        .iter()
        .filter(|row| !row.reachable)
        .map(|row| row.sha.as_str())
        .collect();
    assert_eq!(unreachable, vec!["bbbb3333", "dddd6666"]);

    let surfaces = store.read(|s| store::task_change_surfaces(s, id(20)))?;
    assert_eq!(surfaces.len(), 2);
    let core: TaskChangeSurface = store
        .read(|s| store::task_change_surface(s, id(20), id(10)))?
        .ok_or("surface missing")?;
    assert_eq!(
        (core.base_sha.as_str(), core.final_sha.as_str()),
        ("base1", "final1")
    );
    assert_eq!(core.insertions, 5);
    assert!(
        store
            .read(|s| store::task_change_surface(s, id(21), id(10)))?
            .is_none()
    );
    Ok(())
}
