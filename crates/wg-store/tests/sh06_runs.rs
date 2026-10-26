//! The run family reads (SH-06) against real SQLite: attempts, revision manifests, managed
//! worktrees, resource leases, change sets and items, and artifacts, written through the generic
//! write path and read back through the lookups and lists with the orderings and filters the
//! Python services use.
//!
//! The database is built from `contracts/schema-0011.sql`, as the SH-01, SH-04 and SH-05 checks do.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::entity::{
    AgentAttempt, Artifact, ChangeSet, ChangeSetItem, ManagedWorktree, Repository,
    RepositoryCheckout, ResourceLease, RevisionManifest, Task, Workspace,
};
use wg_model::{JsonObject, Row, Write, derive_id};
use wg_store::{Location, Store};
use wg_types::{
    AccessMode, AttemptStatus, ChangeSetItemStatus, ChangeSetStatus, ErrorCode, Id, TaskKey,
    TaskPriority, TaskStatus, Timestamp, WgError, WorktreeStatus,
};

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
    derive_id(&entropy(6), n)
}

fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 25, 12, 0, second, 0))
}

fn obj(value: Value) -> JsonObject {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
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
        status: TaskStatus::Running,
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

fn checkout(n: u32, repository_id: u32, path: &str) -> RepositoryCheckout {
    RepositoryCheckout {
        id: id(n),
        repository_id: id(repository_id),
        path: path.into(),
        kind: "worktree".into(),
        is_primary: true,
        access_mode: None,
        available: true,
        last_seen_at: None,
        created_at: at(3),
        updated_at: at(3),
    }
}

fn attempt(
    n: u32,
    task_n: u32,
    number: i64,
    status: AttemptStatus,
    provider: &str,
    t: u8,
) -> AgentAttempt {
    AgentAttempt {
        id: id(n),
        workspace_id: id(0),
        task_id: id(task_n),
        number,
        status,
        provider: provider.into(),
        adapter: "stream-json".into(),
        agent_id: None,
        model: None,
        session_id: None,
        parent_attempt_id: None,
        retry_of_attempt_id: None,
        rerun_of_attempt_id: None,
        created_by: "human:brice".into(),
        created_at: at(t),
        queued_at: None,
        started_at: None,
        finished_at: None,
        queue_wait_ms: None,
        duration_ms: None,
        exit_reason: None,
        exit_code: None,
        error: None,
        summary: None,
        input_task_version: Some(1),
        input_task_snapshot: Default::default(),
        prompt_artifact_id: None,
        config_snapshot: Default::default(),
        environment: Default::default(),
        baseline_manifest_id: None,
        outcome_manifest_id: None,
        usage: Default::default(),
        usage_source: None,
        cost_usd: None,
        cost_source: None,
        telemetry: Default::default(),
        modified_repository_ids: VectorSync::new_sync(),
        retries: 0,
        cancel_requested: false,
        updated_at: at(t),
    }
}

fn manifest(n: u32, attempt_n: u32, kind: &str, t: u8) -> RevisionManifest {
    RevisionManifest {
        id: id(n),
        workspace_id: id(0),
        attempt_id: Some(id(attempt_n)),
        kind: kind.into(),
        entries: VectorSync::new_sync(),
        created_at: at(t),
    }
}

fn worktree(n: u32, attempt_n: u32, path: &str, status: WorktreeStatus) -> ManagedWorktree {
    ManagedWorktree {
        id: id(n),
        workspace_id: id(0),
        repository_id: id(70),
        source_checkout_id: id(71),
        attempt_id: Some(id(attempt_n)),
        path: path.into(),
        branch: format!("wg/{n}"),
        base_sha: "a".repeat(40),
        status,
        last_error: None,
        created_at: at(4),
        updated_at: at(4),
        removed_at: None,
    }
}

fn lease(resource_key: &str, holder: &str) -> ResourceLease {
    ResourceLease {
        resource_key: resource_key.into(),
        holder: holder.into(),
        purpose: "run".into(),
        acquired_at: at(5),
        expires_at: None,
    }
}

fn change_set(n: u32, task_n: u32, attempt_n: u32, t: u8) -> ChangeSet {
    ChangeSet {
        id: id(n),
        workspace_id: id(0),
        task_id: id(task_n),
        attempt_id: Some(id(attempt_n)),
        title: format!("change set {n}"),
        status: ChangeSetStatus::Open,
        created_at: at(t),
        updated_at: at(t),
    }
}

fn change_set_item(n: u32, change_set_n: u32, repository_n: u32) -> ChangeSetItem {
    ChangeSetItem {
        id: id(n),
        change_set_id: id(change_set_n),
        repository_id: id(repository_n),
        base_sha: Some("b".repeat(40)),
        head_sha: None,
        branch: Some("wg/x".into()),
        commit_shas: VectorSync::new_sync(),
        status: ChangeSetItemStatus::Pending,
        detail: None,
        updated_at: at(6),
    }
}

fn artifact(
    n: u32,
    task_n: Option<u32>,
    attempt_n: Option<u32>,
    retention: &str,
    t: u8,
) -> Artifact {
    Artifact {
        id: id(n),
        workspace_id: id(0),
        task_id: task_n.map(id),
        attempt_id: attempt_n.map(id),
        kind: "log".into(),
        name: format!("artifact {n}"),
        media_type: "text/plain".into(),
        path: format!("artifacts/{n}.log"),
        size_bytes: 12,
        sha256: "c".repeat(64),
        retention: retention.into(),
        purged_at: None,
        metadata: obj(json!({})),
        created_at: at(t),
    }
}

/// Inserts every row in order, one write each, through the generic write path.
fn insert_all(store: &mut Store, rows: Vec<Row>) -> TestResult {
    store.write(|session| {
        for row in rows {
            session.apply_write(&Write::insert(row))?;
        }
        Ok(())
    })?;
    Ok(())
}

fn numbers(attempts: &[AgentAttempt]) -> Vec<i64> {
    attempts.iter().map(|a| a.number).collect()
}

/// A workspace, a task (`id(1)`, number 1) with its repository, checkout and change-set parents.
fn base_rows() -> Vec<Row> {
    vec![
        Row::Workspace(workspace()),
        Row::Task(task(1, 1)),
        Row::Task(task(2, 2)),
        Row::Repository(repository(70, "core")),
        Row::RepositoryCheckout(checkout(71, 70, "C:/dev/core")),
    ]
}

#[test]
fn attempts_by_task_and_workspace() -> TestResult {
    let mut store = store()?;
    let mut rows = base_rows();
    rows.extend([
        Row::AgentAttempt(attempt(20, 1, 1, AttemptStatus::Failed, "claude-code", 10)),
        Row::AgentAttempt(attempt(
            21,
            1,
            2,
            AttemptStatus::Succeeded,
            "claude-code",
            20,
        )),
        Row::AgentAttempt(attempt(22, 1, 3, AttemptStatus::Running, "command", 30)),
        Row::AgentAttempt(attempt(23, 2, 1, AttemptStatus::Queued, "claude-code", 40)),
    ]);
    insert_all(&mut store, rows)?;

    store.read(|session| {
        // By task, oldest number first; an empty status list does not narrow.
        let all = session.attempts_of_task(id(1), &[])?;
        assert_eq!(numbers(&all), vec![1, 2, 3]);
        let done =
            session.attempts_of_task(id(1), &[AttemptStatus::Succeeded, AttemptStatus::Failed])?;
        assert_eq!(numbers(&done), vec![1, 2]);

        // Workspace list: newest first, and each filter narrows it.
        let newest = session.attempts_in_workspace(id(0), None, &[], None)?;
        let ids: Vec<Id> = newest.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![id(23), id(22), id(21), id(20)]);
        let for_task = session.attempts_in_workspace(id(0), Some(id(1)), &[], None)?;
        assert_eq!(numbers(&for_task), vec![3, 2, 1]);
        let providers = session.attempts_in_workspace(id(0), None, &[], Some("claude-code"))?;
        assert_eq!(
            providers.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![id(23), id(21), id(20)]
        );
        let running =
            session.attempts_in_workspace(id(0), None, &[AttemptStatus::Running], None)?;
        assert_eq!(
            running.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![id(22)]
        );

        let found = session
            .attempt_by_id(id(21))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "attempt"))?;
        assert_eq!(found.number, 2);
        assert_eq!(found.status, AttemptStatus::Succeeded);
        assert!(session.attempt_by_id(id(99))?.is_none());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn manifests_worktrees_and_leases() -> TestResult {
    let mut store = store()?;
    let mut rows = base_rows();
    rows.extend([
        Row::AgentAttempt(attempt(20, 1, 1, AttemptStatus::Running, "claude-code", 10)),
        Row::AgentAttempt(attempt(21, 1, 2, AttemptStatus::Running, "claude-code", 20)),
        Row::RevisionManifest(manifest(30, 20, "outcome", 12)),
        Row::RevisionManifest(manifest(31, 20, "baseline", 11)),
        Row::RevisionManifest(manifest(32, 21, "baseline", 21)),
        Row::ManagedWorktree(worktree(40, 20, "C:/wt/a", WorktreeStatus::Active)),
        Row::ManagedWorktree(worktree(41, 20, "C:/wt/b", WorktreeStatus::Released)),
        Row::ManagedWorktree(worktree(42, 20, "C:/wt/c", WorktreeStatus::Removed)),
        Row::ManagedWorktree(worktree(43, 21, "C:/wt/d", WorktreeStatus::Preparing)),
        Row::ResourceLease(lease("lock:core", "attempt:20")),
        Row::ResourceLease(lease("lock:docs", "attempt:20")),
        Row::ResourceLease(lease("lock:other", "attempt:21")),
    ]);
    insert_all(&mut store, rows)?;

    store.read(|session| {
        // Manifests: oldest first, scoped to one attempt.
        let manifests = session.manifests_of_attempt(id(20))?;
        let kinds: Vec<&str> = manifests.iter().map(|m| m.kind.as_str()).collect();
        assert_eq!(kinds, vec!["baseline", "outcome"]);
        assert_eq!(
            session.manifest_by_id(id(32))?.map(|m| m.attempt_id),
            Some(Some(id(21)))
        );

        // Worktrees: by path, and by attempt narrowed to the statuses the recovery code keeps.
        let by_path = session.worktree_by_path("C:/wt/b")?;
        assert_eq!(by_path.map(|w| w.status), Some(WorktreeStatus::Released));
        assert!(session.worktree_by_path("C:/wt/zzz")?.is_none());
        let all = session.worktrees_of_attempt(id(20), &[])?;
        let paths: Vec<&str> = all.iter().map(|w| w.path.as_str()).collect();
        assert_eq!(paths, vec!["C:/wt/a", "C:/wt/b", "C:/wt/c"]);
        let kept = session.worktrees_of_attempt(
            id(20),
            &[
                WorktreeStatus::Active,
                WorktreeStatus::Preparing,
                WorktreeStatus::Released,
            ],
        )?;
        let kept_paths: Vec<&str> = kept.iter().map(|w| w.path.as_str()).collect();
        assert_eq!(kept_paths, vec!["C:/wt/a", "C:/wt/b"]);
        let in_use =
            session.worktrees_with_status(&[WorktreeStatus::Preparing, WorktreeStatus::Active])?;
        let in_use_paths: Vec<&str> = in_use.iter().map(|w| w.path.as_str()).collect();
        assert_eq!(in_use_paths, vec!["C:/wt/a", "C:/wt/d"]);
        assert_eq!(session.worktrees_with_status(&[])?.len(), 4);

        // Leases: by resource key, and by holder in key order.
        let core = session.lease_by_resource("lock:core")?;
        assert_eq!(core.map(|l| l.holder), Some("attempt:20".to_owned()));
        assert!(session.lease_by_resource("lock:none")?.is_none());
        let held: Vec<String> = session
            .leases_held_by("attempt:20")?
            .into_iter()
            .map(|l: ResourceLease| l.resource_key)
            .collect();
        assert_eq!(held, vec!["lock:core".to_owned(), "lock:docs".to_owned()]);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn change_sets_and_items() -> TestResult {
    let mut store = store()?;
    let mut rows = base_rows();
    rows.extend([
        Row::AgentAttempt(attempt(
            20,
            1,
            1,
            AttemptStatus::Succeeded,
            "claude-code",
            10,
        )),
        Row::ChangeSet(change_set(50, 1, 20, 31)),
        Row::ChangeSet(change_set(51, 1, 20, 30)),
        Row::ChangeSet(change_set(52, 2, 20, 40)),
        Row::ChangeSetItem(change_set_item(60, 50, 70)),
    ]);
    insert_all(&mut store, rows)?;

    store.read(|session| {
        // Change sets of a task: oldest first by created_at.
        let of_task = session.change_sets_of_task(id(1))?;
        assert_eq!(
            of_task.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![id(51), id(50)]
        );
        let of_attempt = session.change_sets_of_attempt(id(20))?;
        assert_eq!(of_attempt.len(), 3);
        assert_eq!(
            session.change_set_by_id(id(52))?.map(|c| c.task_id),
            Some(id(2))
        );

        let items = session.change_set_items(id(50))?;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].repository_id, id(70));
        assert_eq!(items[0].status, ChangeSetItemStatus::Pending);
        assert!(session.change_set_items(id(51))?.is_empty());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn attempts_page_has_total_and_next_offset() -> TestResult {
    let mut store = store()?;
    let mut rows = base_rows();
    rows.extend([
        Row::AgentAttempt(attempt(20, 1, 1, AttemptStatus::Failed, "claude-code", 10)),
        Row::AgentAttempt(attempt(21, 1, 2, AttemptStatus::Succeeded, "command", 20)),
        Row::AgentAttempt(attempt(
            22,
            1,
            3,
            AttemptStatus::Succeeded,
            "claude-code",
            30,
        )),
        Row::AgentAttempt(attempt(23, 1, 4, AttemptStatus::Running, "claude-code", 40)),
        Row::AgentAttempt(attempt(24, 1, 5, AttemptStatus::Queued, "command", 50)),
    ]);
    insert_all(&mut store, rows)?;

    store.read(|session| {
        // Newest first; the total covers the whole filter, not the page.
        let first = session.attempts_page(id(0), None, &[], None, 2, 0)?;
        assert_eq!(first.total, 5);
        assert_eq!(numbers(&first.items), vec![5, 4]);
        assert_eq!(first.next_offset, Some(2));

        let middle = session.attempts_page(id(0), None, &[], None, 2, 2)?;
        assert_eq!(numbers(&middle.items), vec![3, 2]);
        assert_eq!(middle.next_offset, Some(4));

        let last = session.attempts_page(id(0), None, &[], None, 2, 4)?;
        assert_eq!(numbers(&last.items), vec![1]);
        assert_eq!(last.next_offset, None);

        let beyond = session.attempts_page(id(0), None, &[], None, 2, 6)?;
        assert!(beyond.items.is_empty());
        assert_eq!(beyond.total, 5);
        assert_eq!(beyond.next_offset, None);

        // A provider filter changes the total and the paging with it.
        let claude = session.attempts_page(id(0), None, &[], Some("claude-code"), 2, 0)?;
        assert_eq!(claude.total, 3);
        assert_eq!(numbers(&claude.items), vec![4, 3]);
        assert_eq!(claude.next_offset, Some(2));
        let claude_last = session.attempts_page(id(0), None, &[], Some("claude-code"), 2, 2)?;
        assert_eq!(numbers(&claude_last.items), vec![1]);
        assert_eq!(claude_last.next_offset, None);

        // Status and task filters count the same way.
        let succeeded =
            session.attempts_page(id(0), Some(id(1)), &[AttemptStatus::Succeeded], None, 10, 0)?;
        assert_eq!(succeeded.total, 2);
        assert_eq!(numbers(&succeeded.items), vec![3, 2]);
        assert_eq!(succeeded.next_offset, None);
        let other_task = session.attempts_page(id(0), Some(id(2)), &[], None, 10, 0)?;
        assert_eq!(other_task.total, 0);
        assert!(other_task.items.is_empty());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn latest_recorded_head_of_a_task() -> TestResult {
    let mut store = store()?;
    let mut rows = base_rows();
    rows.extend([
        Row::AgentAttempt(attempt(
            20,
            1,
            1,
            AttemptStatus::Succeeded,
            "claude-code",
            10,
        )),
        Row::AgentAttempt(attempt(21, 1, 2, AttemptStatus::Failed, "claude-code", 20)),
        Row::AgentAttempt(attempt(
            22,
            1,
            3,
            AttemptStatus::Succeeded,
            "claude-code",
            30,
        )),
        Row::AgentAttempt(attempt(
            23,
            1,
            4,
            AttemptStatus::Succeeded,
            "claude-code",
            40,
        )),
        Row::ChangeSet(change_set(50, 1, 20, 31)),
        Row::ChangeSet(change_set(51, 1, 21, 40)),
        Row::ChangeSet(change_set(52, 1, 22, 35)),
        Row::ChangeSet(change_set(53, 1, 23, 33)),
        Row::ChangeSetItem(ChangeSetItem {
            head_sha: Some("h1".into()),
            branch: Some("wg/a".into()),
            ..change_set_item(60, 50, 70)
        }),
        // The failed attempt is newer but does not count.
        Row::ChangeSetItem(ChangeSetItem {
            head_sha: Some("h2".into()),
            branch: Some("wg/b".into()),
            ..change_set_item(61, 51, 70)
        }),
        // Newer than 53 but with no head recorded, so it is skipped.
        Row::ChangeSetItem(change_set_item(62, 52, 70)),
        // The newest succeeded change set with a head wins; its branch may be absent.
        Row::ChangeSetItem(ChangeSetItem {
            head_sha: Some("h3".into()),
            branch: None,
            ..change_set_item(63, 53, 70)
        }),
    ]);
    insert_all(&mut store, rows)?;

    store.read(|session| {
        let head = session.latest_recorded_head(id(1), id(70))?;
        assert_eq!(
            head,
            Some(wg_store::RecordedHead {
                head_sha: "h3".into(),
                branch: None,
            })
        );
        // No item for this repository, or no change set for this task.
        assert!(session.latest_recorded_head(id(1), id(75))?.is_none());
        assert!(session.latest_recorded_head(id(2), id(70))?.is_none());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn worktrees_listed_with_repository_names() -> TestResult {
    let mut store = store()?;
    let mut rows = base_rows();
    rows.push(Row::Repository(repository(72, "docs")));
    rows.extend([
        Row::AgentAttempt(attempt(20, 1, 1, AttemptStatus::Running, "claude-code", 10)),
        Row::ManagedWorktree(worktree(40, 20, "C:/wt/a", WorktreeStatus::Active)),
        Row::ManagedWorktree(ManagedWorktree {
            repository_id: id(72),
            created_at: at(7),
            ..worktree(41, 20, "C:/wt/b", WorktreeStatus::Released)
        }),
        Row::ManagedWorktree(ManagedWorktree {
            created_at: at(9),
            ..worktree(42, 20, "C:/wt/c", WorktreeStatus::Preparing)
        }),
    ]);
    insert_all(&mut store, rows)?;

    store.read(|session| {
        // Newest first, each with its repository's name.
        let all = session.worktrees_with_repository(None, None)?;
        let listed: Vec<(&str, &str)> = all
            .iter()
            .map(|w| (w.worktree.path.as_str(), w.repository_name.as_str()))
            .collect();
        assert_eq!(
            listed,
            vec![
                ("C:/wt/c", "core"),
                ("C:/wt/b", "docs"),
                ("C:/wt/a", "core")
            ]
        );

        let active =
            session.worktrees_with_repository(Some(id(0)), Some(WorktreeStatus::Active))?;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].worktree.id, id(40));
        assert_eq!(active[0].repository_name, "core");
        assert_eq!(active[0].worktree.status, WorktreeStatus::Active);

        assert!(
            session
                .worktrees_with_repository(Some(id(99)), None)?
                .is_empty()
        );
        Ok(())
    })?;
    Ok(())
}

#[test]
fn expired_leases_are_swept_at_now() -> TestResult {
    let mut store = store()?;
    insert_all(
        &mut store,
        vec![
            Row::ResourceLease(ResourceLease {
                expires_at: Some(at(10)),
                ..lease("lock:a", "holder-a")
            }),
            Row::ResourceLease(ResourceLease {
                expires_at: Some(at(30)),
                ..lease("lock:b", "holder-b")
            }),
            // No expiry: never swept.
            Row::ResourceLease(lease("lock:c", "holder-c")),
        ],
    )?;

    store.read(|session| {
        let expired = session.expired_leases(at(20))?;
        assert_eq!(
            expired
                .iter()
                .map(|l| l.resource_key.as_str())
                .collect::<Vec<_>>(),
            vec!["lock:a"]
        );
        Ok(())
    })?;

    let swept = store.write(|session| session.sweep_expired_leases(at(20)))?;
    assert_eq!(
        swept
            .iter()
            .map(|l| l.resource_key.as_str())
            .collect::<Vec<_>>(),
        vec!["lock:a"]
    );

    store.read(|session| {
        assert!(session.lease_by_resource("lock:a")?.is_none());
        assert!(session.lease_by_resource("lock:b")?.is_some());
        assert!(session.lease_by_resource("lock:c")?.is_some());
        Ok(())
    })?;

    // The bound is inclusive, and a second sweep at the same time removes nothing.
    assert!(
        store
            .write(|session| session.sweep_expired_leases(at(20)))?
            .is_empty()
    );
    let at_expiry = store.write(|session| session.sweep_expired_leases(at(30)))?;
    assert_eq!(
        at_expiry
            .iter()
            .map(|l| l.resource_key.as_str())
            .collect::<Vec<_>>(),
        vec!["lock:b"]
    );
    store.read(|session| {
        assert!(session.lease_by_resource("lock:c")?.is_some());
        Ok(())
    })?;
    Ok(())
}

#[test]
fn artifacts_by_owner_and_retention() -> TestResult {
    let mut store = store()?;
    let mut rows = base_rows();
    rows.extend([
        Row::AgentAttempt(attempt(
            20,
            1,
            1,
            AttemptStatus::Succeeded,
            "claude-code",
            10,
        )),
        // Artifact 80 is already purged, so the retention scan must skip it.
        Row::Artifact(Artifact {
            purged_at: Some(at(50)),
            ..artifact(80, Some(1), None, "telemetry", 10)
        }),
        Row::Artifact(artifact(81, Some(1), Some(20), "telemetry", 20)),
        Row::Artifact(artifact(82, None, Some(20), "keep", 30)),
        Row::Artifact(artifact(83, Some(1), Some(20), "telemetry", 40)),
    ]);
    insert_all(&mut store, rows)?;

    store.read(|session| {
        let of_task = session.artifacts_of_task(id(1))?;
        assert_eq!(
            of_task.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![id(80), id(81), id(83)]
        );
        let of_attempt = session.artifacts_of_attempt(id(20))?;
        assert_eq!(
            of_attempt.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![id(81), id(82), id(83)]
        );
        assert_eq!(
            session.artifact_by_id(id(82))?.map(|a| a.retention),
            Some("keep".to_owned())
        );

        // Telemetry artifacts created before second 35 that are not purged: only 81.
        let purgeable = session.purgeable_artifacts("telemetry", at(35))?;
        assert_eq!(
            purgeable.iter().map(|a| a.id).collect::<Vec<_>>(),
            vec![id(81)]
        );
        Ok(())
    })?;
    Ok(())
}
