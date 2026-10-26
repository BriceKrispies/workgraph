//! Classification and search checks (SH-11): the classification history and reuse lookup, gold
//! labels, evaluations and the succeeded classifications joined to task keys; the bm25 candidates
//! of `task_fts` with their workspace and overfetch behaviour; the search scope filters (archived,
//! status, category, creation window, project, repository, agent, model, changed path); and the
//! embedding reads.
//!
//! The database is built from `contracts/schema-0011.sql` (tables, the FTS virtual table and the
//! triggers), as `sh04_tasks.rs` builds it. Fixture rows the entity structs do not cover, such as
//! commits and attempts, are inserted with SQL.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::entity::{
    Classification, ClassificationEvaluation, ClassificationGoldLabel, Real, Task, TaskEmbedding,
    Workspace,
};
use wg_model::{JsonObject, Row, Write, derive_id};
use wg_store::{Location, Session, Store};
use wg_storecodec::search_sql::{SearchScope, fts_match};
use wg_types::{Id, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError};

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
    derive_id(&entropy(3), n)
}

fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 25, 12, 0, second, 0))
}

/// The `TIMESTAMP` text the codec writes for `at(second)`.
fn at_text(second: u8) -> String {
    format!("2026-10-25T12:00:{second:02}.000000+00:00")
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

fn sql(store: &mut Store, statements: &str) -> TestResult {
    store.write(|session| session.execute_batch(statements))?;
    Ok(())
}

fn workspace(n: u32, slug: &str) -> Workspace {
    Workspace {
        id: id(n),
        slug: slug.into(),
        name: slug.into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 100,
        settings: obj(json!({})),
        archived_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

/// A task in workspace `ws` (id `n`, key `WG-<number>`).
#[allow(clippy::too_many_arguments)]
fn task(
    ws: u32,
    n: u32,
    number: i64,
    status: TaskStatus,
    title: &str,
    description: &str,
    category: Option<&str>,
    created: u8,
    archived: bool,
) -> Task {
    Task {
        id: id(n),
        workspace_id: id(ws),
        number,
        key: ok(TaskKey::parse(&format!("WG-{number}"))),
        title: title.into(),
        description: description.into(),
        description_hash: "h".into(),
        acceptance_criteria: VectorSync::new_sync(),
        status,
        priority: TaskPriority::Medium,
        category: category.map(str::to_owned),
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
        created_at: at(created),
        updated_at: at(created),
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: archived.then(|| at(9)),
        current_classification_id: None,
    }
}

fn classification(
    n: u32,
    task_n: u32,
    second: u8,
    model: &str,
    status: &str,
    category: &str,
    input_hash: &str,
) -> Classification {
    Classification {
        id: id(n),
        task_id: id(task_n),
        source: "model".into(),
        status: status.into(),
        provider: Some("ollama".into()),
        model: Some(model.into()),
        prompt_version: Some("p1".into()),
        taxonomy_version: "t1".into(),
        input_hash: input_hash.into(),
        category: Some(category.into()),
        subtype: None,
        domain: None,
        affected_systems: VectorSync::new_sync(),
        complexity: None,
        tags: VectorSync::new_sync(),
        confidence: Some(Real(0.9)),
        rationale: None,
        raw_output: None,
        validation_status: "valid".into(),
        error: None,
        duration_ms: None,
        is_current: false,
        confirmed_by: None,
        batch_id: None,
        created_by: "agent:claude".into(),
        created_at: at(second),
    }
}

fn gold(n: u32, ws: u32, task_n: Option<u32>, second: u8) -> ClassificationGoldLabel {
    ClassificationGoldLabel {
        id: id(n),
        workspace_id: id(ws),
        task_id: task_n.map(id),
        title: format!("gold {n}"),
        description: String::new(),
        category: "bug".into(),
        labeled_by: "human:brice".into(),
        created_at: at(second),
    }
}

fn evaluation(n: u32, ws: u32, second: u8) -> ClassificationEvaluation {
    ClassificationEvaluation {
        id: id(n),
        workspace_id: id(ws),
        provider: "ollama".into(),
        model: "m1".into(),
        prompt_version: "p1".into(),
        taxonomy_version: "t1".into(),
        sample_size: 3,
        failures: 0,
        accuracy: Some(Real(0.66)),
        macro_f1: None,
        per_class: obj(json!({})),
        confusion: obj(json!({})),
        results: VectorSync::new_sync(),
        created_at: at(second),
    }
}

fn embedding(n: u32, task_n: u32, model: &str, vector: &[u8]) -> TaskEmbedding {
    TaskEmbedding {
        id: id(n),
        task_id: id(task_n),
        provider: "ollama".into(),
        model: model.into(),
        dimensions: 2,
        vector: vector.iter().copied().collect(),
        input_hash: "h".into(),
        created_at: at(1),
    }
}

/// The ids in `ids` as sorted text, so expectations read as ids and compare as sets.
fn sorted(ids: &[Id]) -> Vec<String> {
    let mut out: Vec<String> = ids.iter().map(Id::to_string).collect();
    out.sort();
    out
}

fn expect_ids(ids: &[Id], wanted: &[u32]) -> TestResult {
    let mut want: Vec<String> = wanted.iter().map(|n| id(*n).to_string()).collect();
    want.sort();
    assert_eq!(sorted(ids), want);
    Ok(())
}

#[test]
fn classification_history_and_reuse_lookup() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace(0, "default"))?;
    put(
        &mut store,
        task(0, 1, 1, TaskStatus::Ready, "a", "", None, 1, false),
    )?;
    put(
        &mut store,
        task(0, 2, 2, TaskStatus::Ready, "b", "", None, 1, false),
    )?;
    put(
        &mut store,
        classification(20, 1, 1, "m1", "succeeded", "bug", "h1"),
    )?;
    put(
        &mut store,
        classification(21, 1, 2, "m1", "succeeded", "feature", "h2"),
    )?;
    put(
        &mut store,
        classification(22, 1, 3, "m1", "failed", "bug", "h1"),
    )?;
    put(
        &mut store,
        classification(23, 2, 4, "m1", "succeeded", "bug", "h1"),
    )?;

    let history = store.read(|s| s.classification_history(id(1)))?;
    let ids: Vec<Id> = history.iter().map(|c| c.id).collect();
    assert_eq!(ids, vec![id(22), id(21), id(20)], "newest first");

    let one = store.read(|s| s.classification(id(20)))?;
    assert_eq!(one.map(|c| c.category), Some(Some("bug".to_owned())));
    assert!(store.read(|s| s.classification(id(99)))?.is_none());

    let reuse = store.read(|s| s.succeeded_classification(id(1), "ollama", "m1", "p1", "h1"))?;
    assert_eq!(
        reuse.map(|c| c.id),
        Some(id(20)),
        "the failed row with the same hash is skipped"
    );
    let other_hash =
        store.read(|s| s.succeeded_classification(id(1), "ollama", "m1", "p1", "zz"))?;
    assert!(other_hash.is_none());
    let other_model =
        store.read(|s| s.succeeded_classification(id(1), "ollama", "m2", "p1", "h1"))?;
    assert!(other_model.is_none());
    Ok(())
}

#[test]
fn gold_labels_and_evaluations_are_per_workspace() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace(0, "default"))?;
    put(&mut store, workspace(9, "other"))?;
    put(
        &mut store,
        task(0, 1, 1, TaskStatus::Ready, "a", "", None, 1, false),
    )?;
    put(&mut store, gold(30, 0, Some(1), 1))?;
    put(&mut store, gold(31, 0, None, 2))?;
    put(&mut store, gold(32, 9, None, 1))?;
    put(&mut store, evaluation(40, 0, 1))?;
    put(&mut store, evaluation(41, 0, 3))?;
    put(&mut store, evaluation(42, 9, 2))?;

    let labels = store.read(|s| s.gold_labels(id(0)))?;
    assert_eq!(
        labels.iter().map(|g| g.id).collect::<Vec<_>>(),
        vec![id(30), id(31)]
    );
    assert_eq!(
        labels[1].task_id, None,
        "a deleted task leaves its label with no task"
    );
    let other = store.read(|s| s.gold_labels(id(9)))?;
    assert_eq!(other.iter().map(|g| g.id).collect::<Vec<_>>(), vec![id(32)]);

    let evals = store.read(|s| s.classification_evaluations(id(0)))?;
    assert_eq!(
        evals.iter().map(|e| e.id).collect::<Vec<_>>(),
        vec![id(41), id(40)],
        "newest first"
    );
    Ok(())
}

#[test]
fn succeeded_classifications_join_their_task_keys() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace(0, "default"))?;
    put(
        &mut store,
        task(0, 1, 10, TaskStatus::Ready, "a", "", None, 1, false),
    )?;
    put(
        &mut store,
        task(0, 2, 11, TaskStatus::Ready, "b", "", None, 1, false),
    )?;
    put(
        &mut store,
        classification(50, 1, 1, "m1", "succeeded", "bug", "h"),
    )?;
    put(
        &mut store,
        classification(51, 1, 2, "m1", "failed", "feature", "h"),
    )?;
    put(
        &mut store,
        classification(52, 2, 3, "m1", "succeeded", "feature", "h"),
    )?;

    let rows = store.read(|s| s.classifications_with_task_keys(id(0)))?;
    let pairs: Vec<(String, Id)> = rows
        .iter()
        .map(|row| (row.task_key.clone(), row.classification.id))
        .collect();
    assert_eq!(
        pairs,
        vec![("WG-10".to_owned(), id(50)), ("WG-11".to_owned(), id(52))]
    );
    Ok(())
}

/// Indexes the task, as the services do after a write.
fn index(store: &mut Store, task_n: u32) -> TestResult {
    store.write(|s| s.index_task(id(task_n)))?;
    Ok(())
}

#[test]
fn fts_candidates_rank_by_bm25_and_stay_in_the_workspace() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace(0, "default"))?;
    put(&mut store, workspace(9, "other"))?;
    // Title hits weigh 10, description hits 3: task 1 must rank above task 2.
    put(
        &mut store,
        task(
            0,
            1,
            1,
            TaskStatus::Ready,
            "billing export",
            "",
            None,
            1,
            false,
        ),
    )?;
    put(
        &mut store,
        task(
            0,
            2,
            2,
            TaskStatus::Ready,
            "reports",
            "billing",
            None,
            1,
            false,
        ),
    )?;
    put(
        &mut store,
        task(9, 3, 1, TaskStatus::Ready, "billing", "", None, 1, false),
    )?;
    index(&mut store, 1)?;
    index(&mut store, 2)?;
    index(&mut store, 3)?;

    let expr = ok_expr(fts_match("billing", true, false))?;
    let hits = store.read(|s| s.fts_candidates(&expr, id(0), 1))?;
    let order: Vec<Id> = hits.iter().map(|h| h.task_id).collect();
    // limit 1 still returns both matches: the query fetches limit * 5 candidates.
    assert_eq!(order, vec![id(1), id(2)]);
    assert!(
        hits[0].rank < hits[1].rank,
        "bm25: lower is better, title hit first"
    );
    assert!(
        hits[0].snippet.contains("[billing]"),
        "snippet: {}",
        hits[0].snippet
    );

    let prefixed = ok_expr(fts_match("bil", true, false))?;
    let prefix_hits = store.read(|s| s.fts_candidates(&prefixed, id(0), 10))?;
    assert_eq!(prefix_hits.len(), 2, "the last token is a prefix match");
    Ok(())
}

fn ok_expr(expr: Option<String>) -> Result<String, Box<dyn Error>> {
    expr.ok_or_else(|| "query has no word characters".into())
}

#[test]
fn search_scope_applies_each_filter() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace(0, "default"))?;
    put(
        &mut store,
        task(0, 1, 1, TaskStatus::Ready, "a", "", Some("bug"), 1, false),
    )?;
    put(
        &mut store,
        task(
            0,
            2,
            2,
            TaskStatus::Accepted,
            "b",
            "",
            Some("feature"),
            2,
            true,
        ),
    )?;
    put(
        &mut store,
        task(0, 3, 3, TaskStatus::Ready, "c", "", None, 3, false),
    )?;
    // A project and a repository that task 1 belongs to.
    sql(
        &mut store,
        &format!(
            "INSERT INTO projects (id, workspace_id, slug, name, description, status, settings, \
             created_at, updated_at) VALUES ('{p}', '{w}', 'core', 'Core', '', 'active', '{{}}', \
             '{t}', '{t}'); \
             INSERT INTO task_projects (task_id, project_id) VALUES ('{t1}', '{p}'); \
             INSERT INTO repositories (id, workspace_id, name, description, tracked_branches, \
             access_mode, health_status, health_detail, is_shallow, metadata, created_at, \
             updated_at) VALUES ('{r}', '{w}', 'wg', '', '[]', 'read', 'ok', '{{}}', 0, '{{}}', \
             '{t}', '{t}'); \
             INSERT INTO task_repositories (task_id, repository_id, access, created_at) \
             VALUES ('{t1}', '{r}', 'write', '{t}');",
            p = id(70),
            r = id(80),
            w = id(0),
            t1 = id(1),
            t = at_text(1),
        ),
    )?;
    // Task 1 has an attempt by claude/opus and a linked commit that touched billing code; task 3
    // has an attempt by codex.
    sql(
        &mut store,
        &format!(
            "INSERT INTO agent_attempts (id, workspace_id, task_id, number, status, provider, \
             adapter, model, created_by, created_at, input_task_snapshot, config_snapshot, \
             environment, usage, telemetry, modified_repository_ids, retries, cancel_requested, \
             updated_at) VALUES \
             ('{a1}', '{w}', '{t1}', 1, 'succeeded', 'claude', 'claude', 'opus', 'human:brice', \
             '{t}', '{{}}', '{{}}', '{{}}', '{{}}', '{{}}', '[]', 0, 0, '{t}'), \
             ('{a2}', '{w}', '{t3}', 1, 'succeeded', 'codex', 'codex', 'gpt', 'human:brice', \
             '{t}', '{{}}', '{{}}', '{{}}', '{{}}', '{{}}', '[]', 0, 0, '{t}'); \
             INSERT INTO commits (id, repository_id, sha, parents, author_name, author_email, \
             committer_name, committer_email, subject, message, trailers, is_merge, insertions, \
             deletions, files_changed, reachable, object_available, structure_analyzed, branches, \
             ingested_at) VALUES ('{c}', '{r}', 'abc', '[]', 'a', 'a@x', 'a', 'a@x', 's', 'm', \
             '[]', 0, 1, 0, 1, 1, 1, 0, '[]', '{t}'); \
             INSERT INTO commit_file_changes (id, commit_id, path, change_type, insertions, \
             deletions, is_binary, file_class, module) VALUES ('{f}', '{c}', \
             'src/billing/export.rs', 'M', 1, 0, 0, 'source', 'billing'); \
             INSERT INTO commit_task_links (id, commit_id, task_id, source, confidence, evidence, \
             created_by, created_at) VALUES ('{l}', '{c}', '{t1}', 'trailer', 1.0, '{{}}', 'git', \
             '{t}');",
            a1 = id(90),
            a2 = id(91),
            c = id(100),
            f = id(101),
            l = id(102),
            r = id(80),
            w = id(0),
            t1 = id(1),
            t3 = id(3),
            t = at_text(1),
        ),
    )?;

    let run = |store: &mut Store, scope: SearchScope| -> Result<Vec<Id>, Box<dyn Error>> {
        Ok(store.read(|s: &mut Session<'_>| s.scoped_task_ids(id(0), &scope))?)
    };

    // Archived tasks are left out unless asked for.
    expect_ids(&run(&mut store, SearchScope::default())?, &[1, 3])?;
    let all = SearchScope {
        include_archived: true,
        ..SearchScope::default()
    };
    expect_ids(&run(&mut store, all.clone())?, &[1, 2, 3])?;

    let category = SearchScope {
        category: Some("bug".into()),
        ..all.clone()
    };
    expect_ids(&run(&mut store, category)?, &[1])?;

    let status = SearchScope {
        statuses: vec![TaskStatus::Accepted.as_str().to_owned()],
        ..all.clone()
    };
    expect_ids(&run(&mut store, status)?, &[2])?;

    let after = SearchScope {
        created_after: Some(at(2)),
        ..all.clone()
    };
    expect_ids(&run(&mut store, after)?, &[2, 3])?;

    let before = SearchScope {
        created_before: Some(at(2)),
        ..SearchScope::default()
    };
    expect_ids(&run(&mut store, before)?, &[1])?;

    let project = SearchScope {
        project_id: Some(id(70)),
        ..all.clone()
    };
    expect_ids(&run(&mut store, project)?, &[1])?;

    let repository = SearchScope {
        repository_id: Some(id(80)),
        ..all.clone()
    };
    expect_ids(&run(&mut store, repository)?, &[1])?;

    let agent = SearchScope {
        agent: Some("codex".into()),
        ..all.clone()
    };
    expect_ids(&run(&mut store, agent)?, &[3])?;

    let model = SearchScope {
        model: Some("opus".into()),
        ..all.clone()
    };
    expect_ids(&run(&mut store, model)?, &[1])?;

    let path = SearchScope {
        changed_path: Some("billing".into()),
        ..all.clone()
    };
    expect_ids(&run(&mut store, path)?, &[1])?;
    let no_path = SearchScope {
        changed_path: Some("nothing".into()),
        ..all
    };
    expect_ids(&run(&mut store, no_path)?, &[])?;
    Ok(())
}

#[test]
fn embedding_reads_are_scoped_by_workspace_provider_and_model() -> TestResult {
    let mut store = store()?;
    put(&mut store, workspace(0, "default"))?;
    put(&mut store, workspace(9, "other"))?;
    put(
        &mut store,
        task(0, 1, 1, TaskStatus::Ready, "a", "", None, 1, false),
    )?;
    put(
        &mut store,
        task(0, 2, 2, TaskStatus::Ready, "b", "", None, 1, false),
    )?;
    put(
        &mut store,
        task(9, 3, 1, TaskStatus::Ready, "c", "", None, 1, false),
    )?;
    let vector = [0u8, 0, 128, 63, 0, 0, 0, 64];
    put(&mut store, embedding(110, 1, "m1", &vector))?;
    put(&mut store, embedding(111, 2, "m2", &vector))?;
    put(&mut store, embedding(112, 3, "m1", &vector))?;

    let vectors = store.read(|s| s.embedding_vectors(id(0), "ollama", "m1"))?;
    assert_eq!(vectors.len(), 1, "only task 1 is in workspace 0 under m1");
    assert_eq!(vectors[0].task_id, id(1));
    assert_eq!(
        vectors[0].vector.iter().copied().collect::<Vec<u8>>(),
        vector.to_vec()
    );

    let found = store.read(|s| s.task_embedding(id(1), "ollama", "m1"))?;
    assert_eq!(found.map(|e| (e.id, e.dimensions)), Some((id(110), 2)));
    assert!(
        store
            .read(|s| s.task_embedding(id(1), "ollama", "m2"))?
            .is_none()
    );
    Ok(())
}
