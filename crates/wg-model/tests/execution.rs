//! wg-model checks for the execution families (`run`, `stack`, `queue`) and the `attempt.*`,
//! `stack.*` and `worktree.*` event payloads: payloads against contracts/events.json, and
//! slices, flow states and value equality on sample rows.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::Value;
use wg_model::entity::{
    AgentAttempt, Artifact, ChangeSet, ChangeSetItem, JobResourceClaim, ManagedWorktree, Real,
    ResourceLease, RevisionManifest, Stack, StackItem, StackRepository, SystemCursor, Worker,
};
use wg_model::event::{
    AttemptCancelRequested, AttemptCreated, AttemptFinished, AttemptMessage,
    AttemptOutcomeCaptured, AttemptProgress, AttemptResult, AttemptStatusChanged,
    AttemptSubagentBound, AttemptUsage, AttemptWorkspacePrepared, StackBisectProbe, StackCreated,
    StackIntegrationConflict, StackIntegrationMerged, StackIntegrationRemoved,
    StackIntegrationSkipped, StackItemAdded, StackItemEjected, StackItemReleased, StackItemRemoved,
    StackItemStateChanged, StackLandRequested, StackLanded, StackLandingRefused,
    StackParentNotReviewable, StackParentRejected, StackRebased, StackRebuilt, StackStatusChanged,
    StackVerificationErrored, StackVerificationFailed, StackVerificationFailedOnBase,
    StackVerificationStarted, StackVerified, WorktreeCreated, WorktreeRecovered, WorktreeRemoved,
};
use wg_model::slice::{QueueSlice, RunSlice, RunState, StackFlow, StackSlice};
use wg_model::{EventPayload, JsonObject, Row, RowKey, StrMap, Table, Write};
use wg_types::{
    AttemptStatus, ChangeSetItemStatus, ChangeSetStatus, Id, JobStatus, StackItemState,
    StackStatus, TaskKey, TaskStatus, Timestamp, WgError, WorktreeStatus,
};

type TestResult = Result<(), Box<dyn Error>>;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn id(n: u128) -> Id {
    Id::from_u128(n)
}

fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 25, 12, 0, second, 0))
}

fn key(text: &str) -> TaskKey {
    ok(TaskKey::parse(text))
}

fn list<T: Clone>(items: &[T]) -> VectorSync<T> {
    items
        .iter()
        .cloned()
        .fold(VectorSync::new_sync(), |list, item| list.push_back(item))
}

fn strmap(pairs: &[(&str, &str)]) -> StrMap {
    StrMap(list(
        &pairs
            .iter()
            .map(|(name, sha)| ((*name).to_owned(), (*sha).to_owned()))
            .collect::<Vec<_>>(),
    ))
}

fn samples() -> Vec<EventPayload> {
    let task_id = id(1);
    let stack = id(3);
    let parent = key("WG-1");
    let child = key("WG-2");
    let extra = JsonObject::new();
    vec![
        EventPayload::AttemptCreated(AttemptCreated {
            task_id,
            task_key: child.clone(),
            number: 1,
            provider: "claude_code".to_owned(),
            adapter: "cli".to_owned(),
            model: None,
            status: AttemptStatus::Queued,
            parent_attempt_id: None,
            retry_of_attempt_id: None,
            rerun_of_attempt_id: None,
        }),
        EventPayload::AttemptStatusChanged(AttemptStatusChanged {
            from: AttemptStatus::Queued,
            to: AttemptStatus::Running,
            task_id,
            extra: extra.clone(),
        }),
        EventPayload::AttemptCancelRequested(AttemptCancelRequested {
            reason: Some("operator".to_owned()),
        }),
        EventPayload::AttemptSucceeded(finished(task_id)),
        EventPayload::AttemptFailed(finished(task_id)),
        EventPayload::AttemptCancelled(finished(task_id)),
        EventPayload::AttemptWorkspacePrepared(AttemptWorkspacePrepared {
            repositories: list(&["repo".to_owned()]),
            bases: strmap(&[("repo", "abc")]),
            isolation: "worktree".to_owned(),
            baseline_manifest_id: id(4),
            change_set_id: None,
        }),
        EventPayload::AttemptOutcomeCaptured(AttemptOutcomeCaptured {
            outcome_manifest_id: id(5),
            modified_repositories: list(&["repo".to_owned()]),
            read_only_violations: list(&[]),
            commits: JsonObject::new(),
        }),
        EventPayload::AttemptSubagentBound(AttemptSubagentBound {
            task_id,
            task_key: child.clone(),
            agent_id: "agent-1".to_owned(),
            agent_type: None,
            model: None,
        }),
        EventPayload::AttemptArtifact(message("artifact")),
        EventPayload::AttemptLog(message("log")),
        EventPayload::AttemptMessage(message("message")),
        EventPayload::AttemptSession(message("session")),
        EventPayload::AttemptToolUse(message("tool_use")),
        EventPayload::AttemptToolResult(message("tool_result")),
        EventPayload::AttemptProgress(AttemptProgress {
            message: "working".to_owned(),
            percent: Some(40),
            extra: extra.clone(),
        }),
        EventPayload::AttemptResult(AttemptResult {
            message: "done".to_owned(),
            status: None,
            summary: None,
            kind: None,
            extra: extra.clone(),
        }),
        EventPayload::AttemptUsage(AttemptUsage {
            message: "tokens".to_owned(),
            input_tokens: None,
            output_tokens: Some(7),
            extra: extra.clone(),
        }),
        EventPayload::StackCreated(StackCreated {
            parent_key: parent.clone(),
            integration_branch: "main".to_owned(),
            base_shas: strmap(&[("repo", "abc")]),
            tasks: list(std::slice::from_ref(&child)),
        }),
        EventPayload::StackItemAdded(StackItemAdded {
            task_key: child.clone(),
            parent_key: parent.clone(),
        }),
        EventPayload::StackItemRemoved(StackItemRemoved {
            task_key: child.clone(),
            parent_key: parent.clone(),
            state: StackItemState::Pending,
        }),
        EventPayload::StackItemReleased(StackItemReleased {
            task_key: child.clone(),
            parent_key: parent.clone(),
            state: StackItemState::Ready,
            rebuild: true,
        }),
        EventPayload::StackItemEjected(StackItemEjected {
            task_key: child.clone(),
            failed_checks: None,
            manual: None,
            position: None,
            reason: None,
            signatures: None,
            verification_run_id: None,
        }),
        EventPayload::StackItemStateChanged(StackItemStateChanged {
            task_key: child.clone(),
            parent_key: parent.clone(),
            from: StackItemState::Pending,
            to: StackItemState::Ready,
            detail: None,
            merge_order: Some(1),
        }),
        EventPayload::StackStatusChanged(StackStatusChanged {
            from: StackStatus::Open,
            to: StackStatus::Integrating,
            reason: "integration started".to_owned(),
        }),
        EventPayload::StackLandRequested(StackLandRequested {
            parent_key: parent.clone(),
            mode: "land".to_owned(),
            reason: "ready".to_owned(),
            base_shas: strmap(&[("repo", "abc")]),
            new_base_shas: strmap(&[("repo", "def")]),
        }),
        EventPayload::StackLanded(StackLanded {
            parent_key: parent.clone(),
            mode: "land".to_owned(),
            reason: "ready".to_owned(),
            run_id: None,
            tips: strmap(&[("repo", "def")]),
            tasks: list(std::slice::from_ref(&child)),
            released: list(&[]),
        }),
        EventPayload::StackLandingRefused(StackLandingRefused {
            mode: "land".to_owned(),
            reason: "not verified".to_owned(),
            error: "stack is open".to_owned(),
        }),
        EventPayload::StackRebased(StackRebased {
            parent_key: parent.clone(),
            reason: "base moved".to_owned(),
            base_shas: strmap(&[("repo", "abc")]),
            new_base_shas: strmap(&[("repo", "def")]),
        }),
        EventPayload::StackRebuilt(StackRebuilt {
            base_shas: strmap(&[("repo", "abc")]),
            tips: strmap(&[("repo", "def")]),
            replayed: list(&["WG-2".to_owned()]),
            conflicted: list(&[]),
        }),
        EventPayload::StackVerificationStarted(StackVerificationStarted {
            tips: strmap(&[("repo", "def")]),
            checks: list(&["unit".to_owned()]),
        }),
        EventPayload::StackVerified(StackVerified {
            run_id: Some(id(6)),
            tips: strmap(&[("repo", "def")]),
            tasks: list(std::slice::from_ref(&child)),
            reason: "all checks passed".to_owned(),
            tolerated: None,
        }),
        EventPayload::StackVerificationFailed(StackVerificationFailed {
            run_id: id(6),
            tips: strmap(&[("repo", "def")]),
            failed_checks: list(&["unit".to_owned()]),
            signatures: JsonObject::new(),
        }),
        EventPayload::StackVerificationFailedOnBase(StackVerificationFailedOnBase {
            run_id: None,
            failed_checks: list(&["unit".to_owned()]),
            reason: "the base fails".to_owned(),
        }),
        EventPayload::StackVerificationErrored(StackVerificationErrored {
            run_id: id(6),
            stage: "verify".to_owned(),
            checks: list(&["unit".to_owned()]),
            excerpt: strmap(&[("unit", "boom")]),
            reason: "check errored".to_owned(),
        }),
        EventPayload::StackBisectProbe(StackBisectProbe {
            position: 2,
            shas: strmap(&[("repo", "abc")]),
            passed: false,
            run_id: id(7),
            failed_checks: list(&["unit".to_owned()]),
        }),
        EventPayload::StackIntegrationMerged(StackIntegrationMerged {
            task_key: child.clone(),
            repository: "repo".to_owned(),
            branch: "integration".to_owned(),
            head: "abc".to_owned(),
            merge_commit: "def".to_owned(),
            already_integrated: false,
        }),
        EventPayload::StackIntegrationConflict(StackIntegrationConflict {
            task_key: child.clone(),
            repository: "repo".to_owned(),
            head: Some("abc".to_owned()),
            integration_tip: None,
            paths: list(&["a.txt".to_owned()]),
            git_error: None,
            instruction: None,
        }),
        EventPayload::StackIntegrationSkipped(StackIntegrationSkipped {
            task_key: child.clone(),
            repository: "repo".to_owned(),
            reason: "no change in this repository".to_owned(),
        }),
        EventPayload::StackIntegrationRemoved(StackIntegrationRemoved {
            repository: "repo".to_owned(),
            branch: "integration".to_owned(),
        }),
        EventPayload::StackParentNotReviewable(StackParentNotReviewable {
            task_key: parent.clone(),
            status: TaskStatus::Running,
            reason: "not awaiting review".to_owned(),
        }),
        EventPayload::StackParentRejected(StackParentRejected {
            task_key: parent.clone(),
            reason: "rejected".to_owned(),
        }),
        EventPayload::WorktreeCreated(WorktreeCreated {
            purpose: None,
            stack_id: Some(stack),
            repository: "repo".to_owned(),
            path: "/wt/a".to_owned(),
            base_sha: "abc".to_owned(),
            base: None,
            branch: None,
        }),
        EventPayload::WorktreeRemoved(WorktreeRemoved {
            path: "/wt/a".to_owned(),
            branch_kept: "wg/branch".to_owned(),
        }),
        EventPayload::WorktreeRecovered(WorktreeRecovered {
            activated: 1,
            orphaned: 0,
            released: 0,
            unknown_dirs: 0,
        }),
    ]
}

fn finished(task_id: Id) -> AttemptFinished {
    AttemptFinished {
        task_id,
        from: AttemptStatus::Running,
        status: AttemptStatus::Succeeded,
        exit_reason: None,
        exit_code: Some(0),
        duration_ms: Some(1200),
        error: None,
    }
}

fn message(text: &str) -> AttemptMessage {
    AttemptMessage {
        message: text.to_owned(),
        extra: JsonObject::new(),
    }
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

fn is_execution(name: &str) -> bool {
    ["attempt.", "stack.", "worktree."]
        .iter()
        .any(|family| name.starts_with(family))
}

#[test]
fn every_execution_event_type_has_a_sample_and_matches_the_contract() -> TestResult {
    let contract = contract_events()?;
    let expected: BTreeSet<&str> = contract
        .keys()
        .map(String::as_str)
        .filter(|name| is_execution(name))
        .collect();
    let known: BTreeSet<&str> = EventPayload::EVENT_TYPES
        .iter()
        .copied()
        .filter(|name| is_execution(name))
        .collect();
    assert_eq!(known, expected, "registry covers the contract's families");
    assert_eq!(known.len(), 45);

    let sampled = samples();
    let sampled_names: BTreeSet<&str> = sampled.iter().map(EventPayload::event_type).collect();
    assert_eq!(sampled_names, known, "every execution event has a sample");

    for payload in sampled {
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
            "{name}: entity type {}",
            payload.entity_type().as_str()
        );
        let data = payload.to_data()?;
        let keys: BTreeSet<String> = data.keys().cloned().collect();
        assert!(
            keys.is_superset(&required),
            "{name}: missing {required:?} in {keys:?}"
        );
        if !open {
            let allowed: BTreeSet<String> = required.union(&optional).cloned().collect();
            assert!(keys.is_subset(&allowed), "{name}: unexpected keys {keys:?}");
        }
        assert_eq!(payload.schema_version(), 1, "{name}");
    }
    Ok(())
}

#[test]
fn optional_keys_are_omitted_and_nullable_keys_are_written_as_null() -> TestResult {
    let progress = EventPayload::AttemptProgress(AttemptProgress {
        message: "m".to_owned(),
        percent: None,
        extra: JsonObject::new(),
    })
    .to_data()?;
    assert!(!progress.contains_key("percent"));
    let created = samples()
        .into_iter()
        .find(|payload| payload.event_type() == "attempt.created")
        .ok_or("no sample")?
        .to_data()?;
    assert_eq!(created.get("model"), Some(&Value::Null));
    assert_eq!(created.get("task_key"), Some(&Value::from("WG-2")));
    Ok(())
}

#[test]
fn execution_tables_are_in_the_table_registry() {
    let execution = [
        Table::AgentAttempt,
        Table::RevisionManifest,
        Table::ManagedWorktree,
        Table::ResourceLease,
        Table::ChangeSet,
        Table::ChangeSetItem,
        Table::Artifact,
        Table::Stack,
        Table::StackRepository,
        Table::StackItem,
        Table::Job,
        Table::JobRun,
        Table::JobResourceClaim,
        Table::Worker,
        Table::SystemCursor,
    ];
    for table in execution {
        assert!(Table::ALL.contains(&table), "{table:?} is listed");
    }
    // 18 base tables (with task_decisions, 19), 15 execution tables, 31 knowledge tables (`lib11_knowledge.rs`).
    assert_eq!(Table::ALL.len(), 65);
    assert_eq!(Table::StackRepository.name(), "stack_repositories");
    assert_eq!(Table::JobResourceClaim.name(), "job_resource_claims");
}

fn attempt_row(n: u128, task: u128, status: AttemptStatus) -> AgentAttempt {
    AgentAttempt {
        id: id(n),
        workspace_id: id(100),
        task_id: id(task),
        number: 1,
        status,
        provider: "claude_code".to_owned(),
        adapter: "cli".to_owned(),
        agent_id: None,
        model: None,
        session_id: None,
        parent_attempt_id: None,
        retry_of_attempt_id: None,
        rerun_of_attempt_id: None,
        created_by: "human:Brice".to_owned(),
        created_at: at(1),
        queued_at: None,
        started_at: None,
        finished_at: None,
        queue_wait_ms: None,
        duration_ms: None,
        exit_reason: None,
        exit_code: None,
        error: None,
        summary: None,
        input_task_version: Some(3),
        input_task_snapshot: JsonObject::new(),
        prompt_artifact_id: None,
        config_snapshot: JsonObject::new(),
        environment: JsonObject::new(),
        baseline_manifest_id: None,
        outcome_manifest_id: None,
        usage: JsonObject::new(),
        usage_source: None,
        cost_usd: Some(Real(0.25)),
        cost_source: Some("measured".to_owned()),
        telemetry: JsonObject::new(),
        modified_repository_ids: list(&[]),
        retries: 0,
        cancel_requested: false,
        updated_at: at(2),
    }
}

#[test]
fn run_slice_applies_writes_and_derives_run_state() {
    let running = attempt_row(10, 1, AttemptStatus::Running);
    let writes = [
        Write::insert(running.clone()),
        Write::insert(RevisionManifest {
            id: id(11),
            workspace_id: id(100),
            attempt_id: Some(id(10)),
            kind: "baseline".to_owned(),
            entries: list(&[JsonObject::new()]),
            created_at: at(1),
        }),
        Write::insert(ResourceLease {
            resource_key: "gpu:0".to_owned(),
            holder: "job-1".to_owned(),
            purpose: "render".to_owned(),
            acquired_at: at(1),
            expires_at: None,
        }),
        Write::insert(ChangeSet {
            id: id(12),
            workspace_id: id(100),
            task_id: id(1),
            attempt_id: Some(id(10)),
            title: "change".to_owned(),
            status: ChangeSetStatus::Open,
            created_at: at(1),
            updated_at: at(1),
        }),
        Write::insert(ChangeSetItem {
            id: id(13),
            change_set_id: id(12),
            repository_id: id(14),
            base_sha: Some("abc".to_owned()),
            head_sha: None,
            branch: None,
            commit_shas: list(&[]),
            status: ChangeSetItemStatus::Pending,
            detail: None,
            updated_at: at(1),
        }),
        Write::insert(Artifact {
            id: id(15),
            workspace_id: id(100),
            task_id: Some(id(1)),
            attempt_id: Some(id(10)),
            kind: "log".to_owned(),
            name: "run.log".to_owned(),
            media_type: "text/plain".to_owned(),
            path: "a/run.log".to_owned(),
            size_bytes: 4,
            sha256: "0".repeat(64),
            retention: "keep".to_owned(),
            purged_at: None,
            metadata: JsonObject::new(),
            created_at: at(1),
        }),
        Write::insert(ManagedWorktree {
            id: id(16),
            workspace_id: id(100),
            repository_id: id(14),
            source_checkout_id: id(17),
            attempt_id: Some(id(10)),
            path: "wt/a".to_owned(),
            branch: "wg/a".to_owned(),
            base_sha: "abc".to_owned(),
            status: WorktreeStatus::Active,
            last_error: None,
            created_at: at(1),
            updated_at: at(1),
            removed_at: None,
        }),
    ];
    let slice = RunSlice::default().apply_all(&writes);
    assert_eq!(slice.attempts_of(id(1)).count(), 1);
    assert_eq!(slice.change_set_items_of(id(12)).count(), 1);
    assert!(slice.lease("gpu:0").is_some());
    assert!(slice.manifest(id(11)).is_some());
    assert!(slice.worktree(id(16)).is_some());
    assert!(slice.artifacts.contains_key(&id(15)));
    assert_eq!(
        slice.change_set(id(12)).map(|set| set.title.as_str()),
        Some("change")
    );

    let state = slice.run_state(id(10)).expect("attempt loaded");
    assert_eq!(state, RunState::of(&running));
    assert!(!state.is_terminal());

    let finished = attempt_row(10, 1, AttemptStatus::Succeeded);
    let after = slice.apply(&Write::update(finished));
    let state = after.run_state(id(10)).expect("attempt loaded");
    assert!(state.is_terminal());
    assert_eq!(state.status, AttemptStatus::Succeeded);
    assert_eq!(
        slice.run_state(id(10)).map(|s| s.status),
        Some(AttemptStatus::Running)
    );

    let without = after.apply(&Write::delete(RowKey::AgentAttempt(id(10))));
    assert!(without.attempt(id(10)).is_none());
    assert!(without.worktree(id(16)).is_some());
    let lease_gone = slice.apply(&Write::delete(RowKey::ResourceLease("gpu:0".to_owned())));
    assert!(lease_gone.lease("gpu:0").is_none());
}

fn stack_row(n: u128, parent: u128, status: StackStatus) -> Stack {
    Stack {
        id: id(n),
        workspace_id: id(100),
        parent_task_id: id(parent),
        status,
        created_at: at(1),
        updated_at: at(1),
        land_request: None,
    }
}

fn item_row(n: u128, stack: u128, task: u128, state: StackItemState, order: i64) -> StackItem {
    StackItem {
        id: id(n),
        stack_id: id(stack),
        task_id: id(task),
        state,
        merge_order: Some(order),
        detail: None,
        created_at: at(1),
        updated_at: at(1),
    }
}

#[test]
fn stack_slice_finds_the_active_stack_and_orders_the_flow() {
    let landed = stack_row(20, 1, StackStatus::Landed);
    let open = stack_row(21, 1, StackStatus::Open);
    let writes = [
        Write::insert(landed.clone()),
        Write::insert(open.clone()),
        Write::insert(StackRepository {
            stack_id: id(21),
            repository_id: id(30),
            base_sha: "abc".to_owned(),
            integration_branch: "integration".to_owned(),
            created_at: at(1),
        }),
        Write::insert(item_row(40, 21, 3, StackItemState::Ready, 2)),
        Write::insert(item_row(41, 21, 2, StackItemState::Merged, 1)),
        Write::insert(StackItem {
            merge_order: None,
            ..item_row(42, 21, 4, StackItemState::Pending, 0)
        }),
    ];
    let slice = StackSlice::default().apply_all(&writes);
    assert_eq!(
        slice.active_stack_of_parent(id(1)).map(|stack| stack.id),
        Some(id(21))
    );
    assert!(slice.active_stack_of_parent(id(9)).is_none());
    assert_eq!(slice.repositories_of(id(21)).count(), 1);
    assert_eq!(slice.items_of(id(21)).count(), 3);

    let flow = slice.flow(id(21)).expect("stack loaded");
    assert_eq!(flow, StackFlow::of(&open, &slice));
    assert_eq!(flow.status, StackStatus::Open);
    assert_eq!(flow.repositories.len(), 1);
    let order: Vec<Id> = flow.items.values().map(|(item, _)| *item).collect();
    assert_eq!(
        order,
        vec![id(41), id(40), id(42)],
        "merge order, unordered last"
    );
    assert!(!flow.all_items_in(StackItemState::Merged));

    let only_landed = StackSlice::default().apply(&Write::insert(landed));
    assert!(only_landed.active_stack_of_parent(id(1)).is_none());

    let removed = slice.apply(&Write::delete(RowKey::StackRepository {
        stack_id: id(21),
        repository_id: id(30),
    }));
    assert_eq!(removed.repositories_of(id(21)).count(), 0);
    let no_stack = slice.apply(&Write::delete(RowKey::Stack(id(21))));
    assert!(no_stack.flow(id(21)).is_none());
}

#[test]
fn queue_slice_holds_jobs_claims_workers_and_cursors() {
    let worker = Worker {
        id: "w-1".to_owned(),
        hostname: "box".to_owned(),
        pid: 4242,
        status: "running".to_owned(),
        concurrency: 2,
        kinds: list(&["stack".to_owned()]),
        current_jobs: list(&[]),
        jobs_completed: 0,
        jobs_failed: 0,
        started_at: at(1),
        heartbeat_at: at(1),
        stopped_at: None,
    };
    let writes = [
        Write::insert(worker.clone()),
        Write::insert(SystemCursor {
            name: "events".to_owned(),
            value: 9,
            updated_at: at(1),
        }),
        Write::insert(JobResourceClaim {
            job_id: id(50),
            resource_key: "repo:b".to_owned(),
        }),
        Write::insert(JobResourceClaim {
            job_id: id(50),
            resource_key: "repo:a".to_owned(),
        }),
        Write::insert(JobResourceClaim {
            job_id: id(51),
            resource_key: "repo:a".to_owned(),
        }),
    ];
    let slice = QueueSlice::default().apply_all(&writes);
    assert_eq!(slice.worker("w-1"), Some(&worker));
    assert_eq!(slice.cursor("events"), Some(9));
    assert_eq!(slice.cursor("missing"), None);
    let claims: Vec<&str> = slice
        .claims_of(id(50))
        .map(|claim| claim.resource_key.as_str())
        .collect();
    assert_eq!(claims, vec!["repo:a", "repo:b"]);
    assert!(slice.job(id(50)).is_none());
    assert_eq!(slice.runs_of(id(50)).count(), 0);
    let released = slice.apply(&Write::delete(RowKey::JobResourceClaim {
        job_id: id(50),
        resource_key: "repo:a".to_owned(),
    }));
    assert_eq!(released.claims_of(id(50)).count(), 1);
    assert_eq!(released.claims_of(id(51)).count(), 1);
    assert_eq!(JobStatus::Queued.as_str(), "queued");
}

#[test]
fn rows_key_and_convert_into_writes() {
    let row = Row::from(attempt_row(10, 1, AttemptStatus::Queued));
    assert_eq!(row.table(), Table::AgentAttempt);
    assert_eq!(row.key(), RowKey::AgentAttempt(id(10)));
    let stack_repo = StackRepository {
        stack_id: id(1),
        repository_id: id(2),
        base_sha: "a".to_owned(),
        integration_branch: "b".to_owned(),
        created_at: at(1),
    };
    let row = Row::from(stack_repo);
    assert_eq!(
        row.key(),
        RowKey::StackRepository {
            stack_id: id(1),
            repository_id: id(2)
        }
    );
    assert_eq!(row.table(), Table::StackRepository);
    let stack = Row::from(stack_row(3, 1, StackStatus::Verified));
    assert_eq!(stack.table(), Table::Stack);
}

#[test]
fn stored_floats_compare_exactly() {
    assert_eq!(Real(1.5), Real(1.5));
    assert_ne!(Real(0.0), Real(-0.0));
    assert_eq!(Real(f64::NAN), Real(f64::NAN));
    let job_seconds = Real(2.0);
    assert_eq!(job_seconds.0, 2.0);
}

#[test]
fn worker_and_resource_keys_are_ordered_by_key() {
    let slice = QueueSlice::default().apply(&Write::insert(SystemCursor {
        name: "b".to_owned(),
        value: 1,
        updated_at: at(1),
    }));
    assert_eq!(slice.cursor("b"), Some(1));
    let lease = ResourceLease {
        resource_key: "k".to_owned(),
        holder: "h".to_owned(),
        purpose: "p".to_owned(),
        acquired_at: at(1),
        expires_at: Some(at(2)),
    };
    assert_eq!(lease.row_key(), RowKey::ResourceLease("k".to_owned()));
}
