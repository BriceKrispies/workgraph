//! wg-model checks: id derivation, event payloads against contracts/events.json, slices as
//! values (clone isolation, replay determinism), writes and decisions.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;

use rpds::{RedBlackTreeSetSync, VectorSync};
use serde_json::{Value, json};
use wg_model::event::{
    ProjectCreated, ProjectRepositoryAdded, ProjectRepositoryRemoved, ProjectUpdated,
    RepositoryAttached, RepositoryChangesDetected, RepositoryCheckoutDiscovered,
    RepositoryCheckoutLost, RepositoryCheckoutRemoved, RepositoryDependencyDeclared,
    RepositoryHealthChanged, RepositoryRemoved, RepositoryUpdated, TaskAccepted,
    TaskChangeSurfaceComputed, TaskClaimed, TaskCommentRecorded, TaskCommitLinked,
    TaskCommitUnlinked, TaskCompleted, TaskCreated, TaskDecomposed, TaskDependencyChanged,
    TaskEscalated, TaskKeyed, TaskRejected, TaskRelationAdded, TaskReleased, TaskReopened,
    TaskRepositoryLinked, TaskRepositoryUnlinked, TaskStatusChanged, TaskSynced, TaskUpdated,
    WorkspaceArchived, WorkspaceCreated, WorkspaceSettingsChanged,
};
use wg_model::{
    Changes, Decision, EventDraft, EventPayload, IdSeq, JsonObject, RowKey, Stamp, StrMap, Table,
    Task, TaskSlice, TaskTag, Workspace, Write, derive_id, py_repr_str,
};
use wg_types::{
    Actor, ActorKind, CommentKind, ErrorCode, Id, Permission, RelationType, TaskKey, TaskPriority,
    TaskStatus, Timestamp, WgError, default_agent_permissions, human_permissions,
};

type TestResult = Result<(), Box<dyn Error>>;

/// Unwraps a fixture value that is valid by construction.
fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 25, 12, 0, second, 0))
}

fn stamp(actor: &str, permissions: &[Permission]) -> Stamp {
    Stamp {
        at: at(0),
        entropy: entropy(7),
        actor: ok(Actor::parse(actor)),
        permissions: permissions.iter().copied().collect(),
        correlation: derive_id(&entropy(99), 0),
        causation: None,
        attempt: None,
        automation_depth: 0,
    }
}

fn human() -> Stamp {
    stamp(
        "human:brice",
        &human_permissions().into_iter().collect::<Vec<_>>(),
    )
}

fn id(n: u32) -> Id {
    derive_id(&entropy(1), n)
}

fn workspace() -> Workspace {
    Workspace {
        id: id(0),
        slug: "default".into(),
        name: "Default".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 3,
        settings: JsonObject::new(),
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
        title: format!("Task {number}"),
        description: String::new(),
        description_hash: String::new(),
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
        metadata: JsonObject::new(),
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

fn tag(task_id: Id, tag: &str) -> TaskTag {
    TaskTag {
        task_id,
        tag: tag.into(),
    }
}

fn base_slice() -> TaskSlice {
    TaskSlice::default().apply_all(&[
        Write::insert(workspace()),
        Write::insert(task(1, 1)),
        Write::insert(task(2, 2)),
        Write::insert(tag(id(1), "zeta")),
        Write::insert(tag(id(1), "alpha")),
        Write::insert(tag(id(2), "beta")),
    ])
}

/// The worked decision of the crate docs, as a replay subject.
fn archive(slice: &TaskSlice, task_id: Id, stamp: &Stamp) -> Result<Decision<Id>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let task = slice
        .task(task_id)
        .ok_or_else(|| WgError::new(ErrorCode::NotFound, "task not found"))?;
    if task.archived_at.is_some() {
        return Ok(Decision::new(task.id));
    }
    let (event_id, _ids) = stamp.ids().next();
    let archived = Task {
        archived_at: Some(stamp.at),
        version: task.version + 1,
        ..task.clone()
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(task.workspace_id),
        task.id,
        EventPayload::TaskArchived(TaskKeyed {
            key: task.key.clone(),
        }),
    );
    Ok(Decision::new(task.id)
        .write(Write::update_versioned(archived, task.version))
        .event(event))
}

// --- ids ---------------------------------------------------------------------

#[test]
fn derived_ids_are_deterministic_version_4_and_distinct() {
    let seed = entropy(0);
    let ids: BTreeSet<Id> = (0..10_000).map(|slot| derive_id(&seed, slot)).collect();
    assert_eq!(ids.len(), 10_000);
    for id in ids.iter().take(50) {
        assert_eq!(id.get_version_num(), 4);
        assert_eq!(id.get_variant(), uuid::Variant::RFC4122);
    }
    assert_eq!(derive_id(&seed, 5), derive_id(&seed, 5));
    assert_ne!(derive_id(&seed, 5), derive_id(&entropy(1), 5));
}

#[test]
fn derived_id_is_pinned() {
    // Changing the derivation changes every id a replayed decision produces; pin it. The value
    // was cross-checked with an independent Python implementation of `derive_id`.
    assert_eq!(
        derive_id(&entropy(0), 0).to_string(),
        "8887347f-3621-4f63-a663-7d3c3ae453fc"
    );
}

#[test]
fn id_sequences_follow_slots() {
    let stamp = human();
    let (first, seq) = stamp.ids().next();
    let ([second, third], seq) = seq.take::<2>();
    assert_eq!(first, stamp.id(0));
    assert_eq!(second, stamp.id(1));
    assert_eq!(third, stamp.id(2));
    assert_eq!(seq.position(), 3);
    let replay: IdSeq = stamp.ids();
    assert_eq!(replay.take::<3>().0, [first, second, third]);
}

// --- stamps --------------------------------------------------------------------

#[test]
fn require_reports_missing_permissions_like_python() {
    let agent = stamp(
        "agent:bot",
        &default_agent_permissions().into_iter().collect::<Vec<_>>(),
    );
    assert!(agent.require(&[Permission::TaskWrite]).is_ok());
    let err = agent
        .require(&[
            Permission::TaskAdmin,
            Permission::TaskAccept,
            Permission::TaskRead,
        ])
        .expect_err("agent lacks accept and admin");
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert_eq!(
        err.message,
        "actor 'agent:bot' lacks permission(s): task:accept, task:admin"
    );
    assert_eq!(
        Value::Object(err.details),
        json!({"actor": "agent:bot", "missing": ["task:accept", "task:admin"]})
    );
}

#[test]
fn py_repr_str_matches_python() {
    assert_eq!(py_repr_str("human:brice"), "'human:brice'");
    assert_eq!(py_repr_str("it's"), "\"it's\"");
    assert_eq!(py_repr_str("a'b\"c"), "'a\\'b\"c'");
    assert_eq!(py_repr_str("tab\there\\"), "'tab\\there\\\\'");
    assert_eq!(py_repr_str("\u{1}é"), "'\\x01é'");
}

// --- events against the contract ---------------------------------------------------

fn changes() -> Changes {
    Changes::new().with("title", json!("old"), json!("new"))
}

fn key(text: &str) -> TaskKey {
    ok(TaskKey::parse(text))
}

/// One sample per event type.
fn samples() -> Vec<EventPayload> {
    let keyed = || TaskKeyed { key: key("WG-1") };
    let comment = || TaskCommentRecorded {
        comment_id: id(9),
        kind: CommentKind::Comment,
        excerpt: "hello".into(),
        extra: JsonObject::new(),
    };
    let dependency = || TaskDependencyChanged {
        depends_on: id(2),
        depends_on_key: key("WG-2"),
    };
    let attached = || RepositoryAttached {
        name: "app".into(),
        path: "C:/src/app".into(),
        kind: "clone".into(),
        matched_by: None,
        fingerprint: Some("abc".into()),
        remotes: StrMap(
            [(
                "origin".to_owned(),
                "https://example.com/app.git".to_owned(),
            )]
            .into_iter()
            .collect(),
        ),
    };
    vec![
        EventPayload::WorkspaceCreated(WorkspaceCreated {
            slug: "default".into(),
            name: "Default".into(),
            key_prefix: "WG".into(),
        }),
        EventPayload::WorkspaceSettingsChanged(WorkspaceSettingsChanged {
            before: JsonObject::new(),
            after: JsonObject::new(),
        }),
        EventPayload::WorkspaceArchived(WorkspaceArchived {}),
        EventPayload::ProjectCreated(ProjectCreated {
            slug: "web".into(),
            name: "Web".into(),
            repository_ids: VectorSync::new_sync().push_back(id(3)),
        }),
        EventPayload::ProjectUpdated(ProjectUpdated { changes: changes() }),
        EventPayload::ProjectRepositoryAdded(ProjectRepositoryAdded {
            repository_id: id(3),
            role: "member".into(),
        }),
        EventPayload::ProjectRepositoryRemoved(ProjectRepositoryRemoved {
            repository_id: id(3),
        }),
        EventPayload::RepositoryRegistered(attached()),
        EventPayload::RepositoryCheckoutAdded(attached()),
        EventPayload::RepositoryCheckoutDiscovered(RepositoryCheckoutDiscovered {
            path: "C:/src/app-wt".into(),
            kind: "worktree".into(),
            branch: Some(Some("feature".into())),
            detached: Some(false),
            locked: Some(false),
            source: Some("register".into()),
            reason: None,
        }),
        EventPayload::RepositoryCheckoutLost(RepositoryCheckoutLost {
            path: "C:/src/app".into(),
            kind: "clone".into(),
            reason: Some("missing".into()),
        }),
        EventPayload::RepositoryCheckoutRemoved(RepositoryCheckoutRemoved {
            path: "C:/src/app".into(),
        }),
        EventPayload::RepositoryChangesDetected(RepositoryChangesDetected {
            changed_refs: VectorSync::new_sync().push_back("refs/heads/main".to_owned()),
            removed_refs: VectorSync::new_sync(),
            non_fast_forward: false,
        }),
        EventPayload::RepositoryDependencyDeclared(RepositoryDependencyDeclared {
            depends_on: id(4),
            depends_on_name: "lib".into(),
        }),
        EventPayload::RepositoryHealthChanged(RepositoryHealthChanged {
            from: "ok".into(),
            to: "degraded".into(),
            detail: JsonObject::new(),
        }),
        EventPayload::RepositoryRemoved(RepositoryRemoved {
            name: "app".into(),
            note: "registration archived; files on disk untouched".into(),
        }),
        EventPayload::RepositoryUpdated(RepositoryUpdated { changes: changes() }),
        EventPayload::TaskCreated(TaskCreated {
            key: key("WG-1"),
            title: "Title".into(),
            description: String::new(),
            acceptance_criteria: VectorSync::new_sync(),
            status: TaskStatus::Backlog,
            priority: TaskPriority::Medium,
            tags: VectorSync::new_sync(),
            project_ids: VectorSync::new_sync(),
            repository_ids: VectorSync::new_sync(),
            parent_id: None,
            depends_on: VectorSync::new_sync(),
            assignee: None,
            auto_execute: false,
        }),
        EventPayload::TaskUpdated(TaskUpdated {
            key: key("WG-1"),
            changes: changes(),
            material_change: true,
            reason: None,
            version: 2,
        }),
        EventPayload::TaskStatusChanged(TaskStatusChanged {
            from: TaskStatus::Backlog,
            to: TaskStatus::Ready,
            reason: None,
            version: 2,
            attempt_id: None,
            run_id: None,
            extra: JsonObject::new(),
        }),
        EventPayload::TaskReopened(TaskReopened {
            from: TaskStatus::Accepted,
            to: TaskStatus::Ready,
            reason: Some("again".into()),
        }),
        EventPayload::TaskBecameReady(keyed()),
        EventPayload::TaskArchived(keyed()),
        EventPayload::TaskUnarchived(keyed()),
        EventPayload::TaskClaimed(TaskClaimed {
            claimed_by: "agent:bot".into(),
            expires_at: at(30),
        }),
        EventPayload::TaskReleased(TaskReleased {
            previous_claimant: Some("agent:bot".into()),
            reason: None,
        }),
        EventPayload::TaskCompleted(TaskCompleted {
            summary: "done".into(),
            next_status: TaskStatus::AwaitingReview,
            verification_configured: false,
        }),
        EventPayload::TaskCommentAdded(comment()),
        EventPayload::TaskProgressReported(comment()),
        EventPayload::TaskResultReported(comment()),
        EventPayload::TaskDependencyAdded(dependency()),
        EventPayload::TaskDependencyRemoved(dependency()),
        EventPayload::TaskRelationAdded(TaskRelationAdded {
            target: id(2),
            target_key: key("WG-2"),
            relation_type: RelationType::RelatesTo,
        }),
        EventPayload::TaskRepositoryLinked(TaskRepositoryLinked {
            repository_id: id(3),
            repository: "app".into(),
            access: "read_write".into(),
        }),
        EventPayload::TaskRepositoryUnlinked(TaskRepositoryUnlinked {
            repository_id: id(3),
            repository: "app".into(),
        }),
        EventPayload::TaskAccepted(TaskAccepted {
            mode: "manual".into(),
            reason: "looks good".into(),
            verification_run_id: None,
            manifest_id: None,
        }),
        EventPayload::TaskRejected(TaskRejected {
            reason: "no".into(),
        }),
        EventPayload::TaskEscalated(TaskEscalated {
            failed_attempts: 3,
            limit: 3,
            reason: "attempt limit reached".into(),
        }),
        EventPayload::TaskDecomposed(TaskDecomposed {
            children: VectorSync::new_sync().push_back(key("WG-5")),
            provider: "heuristic".into(),
            model: "none".into(),
            rationale: "split".into(),
        }),
        EventPayload::TaskSynced(TaskSynced {
            connection: "jira".into(),
            external_key: "ABC-1".into(),
            created: true,
            changed_fields: VectorSync::new_sync(),
        }),
        EventPayload::TaskCommitLinked(TaskCommitLinked {
            commit_id: id(8),
            sha: "deadbeef".into(),
            repository_id: id(3),
            source: "explicit".into(),
        }),
        EventPayload::TaskCommitUnlinked(TaskCommitUnlinked {
            sha: "deadbeef".into(),
            removed: 1,
        }),
        EventPayload::TaskChangeSurfaceComputed(TaskChangeSurfaceComputed {
            repositories: 1,
            insertions: 10,
            deletions: 2,
        }),
    ]
}

fn contract_events() -> Result<serde_json::Map<String, Value>, Box<dyn Error>> {
    let text = std::fs::read_to_string(repo_root().join("contracts/events.json"))?;
    let doc: Value = serde_json::from_str(&text)?;
    Ok(doc
        .get("events")
        .and_then(Value::as_object)
        .cloned()
        .ok_or("contracts/events.json has no events object")?)
}

fn strings(value: Option<&Value>) -> BTreeSet<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn every_family_event_of_the_contract_has_a_payload() -> TestResult {
    // Every family present in the contract: the registry must name exactly the contract's events.
    let contract = contract_events()?;
    let expected: BTreeSet<&str> = contract.keys().map(String::as_str).collect();
    let known: BTreeSet<&str> = EventPayload::EVENT_TYPES.iter().copied().collect();
    assert_eq!(known, expected, "registry covers the contract's families");
    assert_eq!(
        known.len(),
        EventPayload::EVENT_TYPES.len(),
        "duplicate type"
    );
    // The samples below cover the workspace, project, repository and task families. The other
    // families are sampled by `lib11_knowledge.rs` and `execution.rs`.
    let families = ["workspace.", "project.", "repository.", "task."];
    let core_known: BTreeSet<&str> = known
        .iter()
        .copied()
        .filter(|name| families.iter().any(|family| name.starts_with(family)))
        .collect();
    assert_eq!(core_known.len(), 43);
    let sampled: BTreeSet<&str> = samples().iter().map(EventPayload::event_type).collect();
    assert_eq!(sampled, core_known, "every event type has a sample");
    Ok(())
}

#[test]
fn payload_keys_and_entity_types_match_the_contract() -> TestResult {
    let contract = contract_events()?;
    for payload in samples() {
        let name = payload.event_type();
        let spec = contract
            .get(name)
            .ok_or(format!("{name} not in contract"))?;
        let required = strings(spec.get("data_keys"));
        let optional = strings(spec.get("optional_data_keys"));
        let open = spec.get("data_open").and_then(Value::as_bool) == Some(true);
        let entities = strings(spec.get("entity_types"));
        assert!(
            entities.contains(payload.entity_type().as_str()),
            "{name}: entity type"
        );
        let data = payload.to_data()?;
        let keys: BTreeSet<String> = data.keys().cloned().collect();
        assert!(keys.is_superset(&required), "{name}: missing keys");
        if !open {
            let allowed: BTreeSet<String> = required.union(&optional).cloned().collect();
            assert!(keys.is_subset(&allowed), "{name}: unexpected keys {keys:?}");
        }
        let draft = EventDraft::new(&human(), id(50), Some(id(0)), id(1), payload.clone());
        assert_eq!(draft.event_type(), name);
        assert_eq!(
            draft.schema_version(),
            if name == "task.created" { 2 } else { 1 }
        );
    }
    Ok(())
}

#[test]
fn task_created_data_is_pythons_dict() -> TestResult {
    let payload = EventPayload::TaskCreated(TaskCreated {
        key: key("WG-7"),
        title: "Ship it".into(),
        description: "body".into(),
        acceptance_criteria: VectorSync::new_sync().push_back("works".to_owned()),
        status: TaskStatus::Backlog,
        priority: TaskPriority::High,
        tags: VectorSync::new_sync().push_back("a".to_owned()),
        project_ids: VectorSync::new_sync().push_back(id(3)),
        repository_ids: VectorSync::new_sync(),
        parent_id: None,
        depends_on: VectorSync::new_sync().push_back(id(2)),
        assignee: Some("brice".into()),
        auto_execute: true,
    });
    let text = serde_json::to_string(&Value::Object(payload.to_data()?))?;
    let expected = json!({
        "key": "WG-7",
        "title": "Ship it",
        "description": "body",
        "acceptance_criteria": ["works"],
        "status": "backlog",
        "priority": "high",
        "tags": ["a"],
        "project_ids": [id(3).to_string()],
        "repository_ids": [],
        "parent_id": null,
        "depends_on": [id(2).to_string()],
        "assignee": "brice",
        "auto_execute": true,
    });
    assert_eq!(text, serde_json::to_string(&expected)?);
    Ok(())
}

#[test]
fn optional_and_open_keys_follow_python() -> TestResult {
    let changed = |attempt_id, extra: JsonObject| {
        EventPayload::TaskStatusChanged(TaskStatusChanged {
            from: TaskStatus::Running,
            to: TaskStatus::AwaitingReview,
            reason: Some("completion reported".into()),
            version: 4,
            attempt_id,
            run_id: None,
            extra,
        })
        .to_data()
    };
    let absent = changed(None, JsonObject::new())?;
    assert!(!absent.contains_key("attempt_id"));
    assert!(!absent.contains_key("run_id"));
    let null = changed(Some(None), JsonObject::new())?;
    assert_eq!(null.get("attempt_id"), Some(&Value::Null));
    // `{..., **extra}`: a merged key replaces the fixed one in place.
    let extra: JsonObject = [
        ("reason".to_owned(), json!("overridden")),
        ("x".to_owned(), json!(1)),
    ]
    .into_iter()
    .collect();
    let merged = changed(None, extra)?;
    let keys: Vec<&str> = merged.keys().map(String::as_str).collect();
    assert_eq!(keys, ["from", "to", "reason", "version", "x"]);
    assert_eq!(merged.get("reason"), Some(&json!("overridden")));
    Ok(())
}

#[test]
fn claimed_expiry_uses_isoformat_and_changes_are_pairs() -> TestResult {
    let claimed = EventPayload::TaskClaimed(TaskClaimed {
        claimed_by: "agent:bot".into(),
        expires_at: at(30),
    })
    .to_data()?;
    assert_eq!(
        claimed.get("expires_at"),
        Some(&json!("2026-10-25T12:00:30+00:00"))
    );
    let updated = EventPayload::ProjectUpdated(ProjectUpdated {
        changes: Changes::new().with("slug", json!("a"), json!("b")).with(
            "name",
            json!("A"),
            json!("B"),
        ),
    })
    .to_data()?;
    assert_eq!(
        serde_json::to_string(&Value::Object(updated))?,
        r#"{"changes":{"slug":["a","b"],"name":["A","B"]}}"#
    );
    Ok(())
}

#[test]
fn event_drafts_carry_the_stamp_and_the_automation_mark() -> TestResult {
    let base = human();
    let auto = Stamp {
        actor: Actor::new(ActorKind::Automation, "promote")?,
        automation_depth: 2,
        attempt: Some(id(60)),
        ..base.clone()
    };
    let payload = EventPayload::TaskBecameReady(TaskKeyed { key: key("WG-1") });
    let draft = EventDraft::new(&auto, id(50), Some(id(0)), id(1), payload.clone())
        .caused_by(id(51))
        .with_attempt(None);
    assert_eq!(draft.causation_id, Some(id(51)));
    assert_eq!(draft.attempt_id, Some(id(60)));
    assert_eq!(draft.correlation_id, base.correlation);
    assert_eq!(draft.occurred_at, base.at);
    assert_eq!(
        Value::Object(draft.data()?),
        json!({"key": "WG-1", "_automation": {"depth": 2, "actor": "automation:promote"}})
    );
    let plain = EventDraft::new(&base, id(50), None, id(1), payload).with_attempt(Some(id(61)));
    assert_eq!(plain.attempt_id, Some(id(61)));
    assert_eq!(Value::Object(plain.data()?), json!({"key": "WG-1"}));
    Ok(())
}

// --- slices, writes, decisions -----------------------------------------------------

#[test]
fn slice_clones_are_isolated() {
    let original = base_slice();
    let snapshot = original.clone();
    let changed = original
        .clone()
        .apply(&Write::update(Task {
            title: "changed".into(),
            ..task(1, 1)
        }))
        .apply(&Write::delete(RowKey::TaskTag {
            task_id: id(1),
            tag: "zeta".into(),
        }))
        .apply(&Write::insert(tag(id(2), "gamma")));
    assert_eq!(original, snapshot);
    assert_eq!(
        original.task(id(1)).map(|t| t.title.as_str()),
        Some("Task 1")
    );
    assert_eq!(
        changed.task(id(1)).map(|t| t.title.as_str()),
        Some("changed")
    );
    assert_eq!(changed.tags_of(id(1)).collect::<Vec<_>>(), ["alpha"]);
    assert_eq!(
        changed.tags_of(id(2)).collect::<Vec<_>>(),
        ["beta", "gamma"]
    );
    assert_eq!(
        original.tags_of(id(1)).collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
}

#[test]
fn registry_rows_reach_the_embedded_registry_slice() {
    let slice = base_slice();
    assert_eq!(
        slice.registry.workspace(id(0)).map(|w| w.slug.as_str()),
        Some("default")
    );
    let gone = slice.apply(&Write::delete(RowKey::Workspace(id(0))));
    assert!(gone.registry.workspace(id(0)).is_none());
    assert!(gone.task(id(1)).is_some());
    assert_eq!(
        slice.task_by_key(id(0), &key("WG-2")).map(|t| t.id),
        Some(id(2))
    );
}

#[test]
fn decisions_replay_identically() -> TestResult {
    let slice = base_slice();
    let stamp = human();
    let first = archive(&slice, id(1), &stamp)?;
    let second = archive(&slice.clone(), id(1), &stamp.clone())?;
    assert_eq!(first, second);
    assert_eq!(first.writes.len(), 1);
    let after = slice.apply_all(first.writes.iter());
    assert_eq!(
        after.task(id(1)).and_then(|t| t.archived_at),
        Some(stamp.at)
    );
    assert!(archive(&after, id(1), &stamp)?.is_noop());
    let event = first.events.first().ok_or("no event")?;
    assert_eq!(event.id, stamp.id(0));
    assert_eq!(event.event_type(), "task.archived");
    Ok(())
}

#[test]
fn decisions_compose_in_order() {
    let one = Decision::new(())
        .write(Write::insert(tag(id(1), "a")))
        .write(Write::insert(tag(id(1), "b")));
    let two = Decision::new(7).write(Write::delete(RowKey::Task(id(2))));
    let joined = one.then(two).map(|n| n + 1);
    assert_eq!(joined.reply, 8);
    let tables: Vec<Table> = joined.writes.iter().map(Write::table).collect();
    assert_eq!(tables, [Table::TaskTag, Table::TaskTag, Table::Task]);
    assert_eq!(
        joined.writes.get(1).map(Write::key),
        Some(RowKey::TaskTag {
            task_id: id(1),
            tag: "b".into()
        })
    );
}

#[test]
fn table_names_are_schema_tables() -> TestResult {
    let schema = std::fs::read_to_string(repo_root().join("contracts/schema-0010.sql"))?;
    let names: BTreeSet<&str> = Table::ALL.iter().map(|table| table.name()).collect();
    assert_eq!(names.len(), Table::ALL.len());
    for name in names {
        let quoted = format!("CREATE TABLE \"{name}\" (");
        assert!(
            schema.contains(&format!("CREATE TABLE {name} (")) || schema.contains(&quoted),
            "{name} is not a table of schema 0010"
        );
    }
    Ok(())
}

#[test]
fn permissions_set_is_persistent() {
    let stamp = human();
    let fewer = Stamp {
        permissions: stamp.permissions.remove(&Permission::TaskAccept),
        ..stamp.clone()
    };
    assert!(stamp.has(Permission::TaskAccept));
    assert!(!fewer.has(Permission::TaskAccept));
    let empty: RedBlackTreeSetSync<Permission> = RedBlackTreeSetSync::new_sync();
    assert!(empty.is_empty());
}
