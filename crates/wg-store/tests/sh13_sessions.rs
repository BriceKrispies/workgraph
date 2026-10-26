//! Session-family checks (SH-13): agent sessions and their paged summaries, session bindings and
//! unbinding, subagent runs with the stopped-run join, internal tasks, hook and tool events, the
//! open-attempt lookup for subagents, and the retention compaction selections.
//!
//! The database is built from `contracts/schema-0011.sql` (tables, the FTS virtual table and the
//! triggers), as `sh04_tasks.rs` builds it.

use std::error::Error;
use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::entity::{
    AgentAttempt, AgentInternalTask, AgentSession, Artifact, HookEvent, SessionBinding,
    SubagentRun, Task, ToolEvent, Workspace,
};
use wg_model::{JsonObject, Row, Write, derive_id};
use wg_store::{Location, Store};
use wg_types::{AttemptStatus, Id, TaskPriority, TaskStatus, Timestamp, WgError};

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

/// A timestamp `second` seconds past noon on the fixture day, plus `micros`.
fn at(second: u8, micros: u32) -> Timestamp {
    ok(Timestamp::from_components(
        2026, 10, 25, 12, 0, second, micros,
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
        created_at: at(0, 0),
        updated_at: at(0, 0),
    }
}

fn task(n: u32) -> Task {
    Task {
        id: id(n),
        workspace_id: id(0),
        number: 1,
        key: ok(wg_types::TaskKey::parse("WG-1")),
        title: "Task".into(),
        description: "d".into(),
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
        created_at: at(1, 0),
        updated_at: at(1, 0),
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    }
}

/// An agent session of the `claude_code` provider in the fixture workspace.
fn session(n: u32, external: &str, created_second: u8, workspace: Option<Id>) -> AgentSession {
    AgentSession {
        id: id(n),
        provider: "claude_code".into(),
        external_session_id: external.into(),
        workspace_id: workspace,
        attempt_id: None,
        task_id: None,
        binding_source: "unbound".into(),
        cwd: Some("C:/dev/x".into()),
        model: None,
        transcript_path: None,
        started_at: None,
        ended_at: None,
        end_reason: None,
        tool_calls: i64::from(n),
        tool_failures: 0,
        created_at: at(created_second, 0),
        updated_at: at(created_second, 0),
    }
}

fn subagent(n: u32, session_n: u32, external: &str, stopped: Option<Timestamp>) -> SubagentRun {
    SubagentRun {
        id: id(n),
        session_id: id(session_n),
        external_agent_id: external.into(),
        agent_type: Some("Explore".into()),
        attempt_id: None,
        started_at: None,
        stopped_at: stopped,
        last_message_excerpt: None,
        transcript_path: None,
        requested_model: None,
        model: None,
        usage: None,
        telemetry: None,
        task_id: None,
    }
}

fn hook(n: u32, dedupe: &str, name: &str, received: Timestamp) -> HookEvent {
    HookEvent {
        id: id(n),
        dedupe_key: dedupe.into(),
        provider: "claude_code".into(),
        hook_event_name: name.into(),
        external_session_id: None,
        session_id: None,
        attempt_id: None,
        payload: Some(obj(json!({"n": n}))),
        payload_purged_at: None,
        received_at: received,
        source: "direct".into(),
    }
}

fn artifact(n: u32, retention: &str, created: Timestamp) -> Artifact {
    Artifact {
        id: id(n),
        workspace_id: id(0),
        task_id: None,
        attempt_id: None,
        kind: "log".into(),
        name: format!("log-{n}"),
        media_type: "text/plain".into(),
        path: format!("log-{n}.txt"),
        size_bytes: 10,
        sha256: "0".repeat(64),
        retention: retention.into(),
        purged_at: None,
        metadata: obj(json!({})),
        created_at: created,
    }
}

/// Inserts the workspace and task every other fixture hangs off.
fn base(store: &mut Store) -> TestResult {
    put(store, workspace())?;
    put(store, task(10))?;
    Ok(())
}

#[test]
fn agent_sessions_page_newest_first_and_filter_by_workspace() -> TestResult {
    let mut store = store()?;
    base(&mut store)?;
    put(&mut store, session(20, "s-old", 1, Some(id(0))))?;
    put(&mut store, session(21, "s-mid", 2, Some(id(0))))?;
    put(&mut store, session(22, "s-new", 3, None))?;

    let (first_page, second_page, in_ws, found) = store.read(|s| {
        Ok((
            s.agent_session_summaries("claude_code", None, 2, 0)?,
            s.agent_session_summaries("claude_code", None, 2, 2)?,
            s.agent_session_summaries("claude_code", Some(id(0)), 10, 0)?,
            s.agent_session_by_external("claude_code", "s-mid")?,
        ))
    })?;
    assert_eq!(first_page.len(), 2);
    assert_eq!(second_page.len(), 1);
    assert_eq!(in_ws.len(), 2);
    assert_eq!(first_page[0].external_session_id, "s-new");
    assert_eq!(first_page[1].external_session_id, "s-mid");
    assert_eq!(second_page[0].external_session_id, "s-old");
    assert_eq!(in_ws[0].external_session_id, "s-mid");
    assert_eq!(in_ws[1].external_session_id, "s-old");
    assert_eq!(first_page[0].tool_calls, 22);
    assert_eq!(found.map(|row| row.id), Some(id(21)));
    Ok(())
}

#[test]
fn subagent_runs_and_internal_tasks_by_external_key() -> TestResult {
    let mut store = store()?;
    base(&mut store)?;
    put(&mut store, session(20, "s-1", 1, Some(id(0))))?;
    put(&mut store, subagent(30, 20, "agent-a", None))?;
    put(&mut store, subagent(31, 20, "agent-b", Some(at(5, 0))))?;
    put(
        &mut store,
        AgentInternalTask {
            id: id(40),
            session_id: id(20),
            external_task_id: "t-1".into(),
            subject: "Write tests".into(),
            description: None,
            status: "pending".into(),
            task_id: None,
            external_agent_id: None,
            created_at: at(2, 0),
            completed_at: None,
        },
    )?;

    let (run, missing, count, internal) = store.read(|s| {
        Ok((
            s.subagent_run_by_external(id(20), "agent-b")?,
            s.subagent_run_by_external(id(20), "agent-z")?,
            s.subagent_run_count_for_session(id(20))?,
            s.internal_task_by_external(id(20), "t-1")?,
        ))
    })?;
    assert_eq!(run.map(|row| row.id), Some(id(31)));
    assert!(missing.is_none());
    assert_eq!(count, 2);
    assert_eq!(internal.map(|row| row.subject), Some("Write tests".into()));
    Ok(())
}

#[test]
fn bindings_resolve_active_only_and_unbind_by_precedence() -> TestResult {
    let mut store = store()?;
    base(&mut store)?;
    let binding = |n: u32, external: Option<&str>, cwd: Option<&str>, active: bool, second: u8| {
        SessionBinding {
            id: id(n),
            workspace_id: id(0),
            task_id: id(10),
            provider: "claude_code".into(),
            external_session_id: external.map(str::to_owned),
            cwd_prefix: cwd.map(str::to_owned),
            active,
            created_by: "human:brice".into(),
            created_at: at(second, 0),
        }
    };
    put(&mut store, binding(50, Some("ext-1"), None, true, 1))?;
    put(&mut store, binding(51, Some("ext-1"), None, false, 2))?;
    put(&mut store, binding(52, None, Some("C:/dev/a"), true, 3))?;
    put(&mut store, binding(53, None, Some("C:/dev/b"), true, 4))?;
    put(&mut store, binding(54, None, Some("C:/dev/c"), false, 5))?;

    let (by_ext, cwd_rows) = store.read(|s| {
        Ok((
            s.active_binding_by_external("ext-1")?,
            s.active_cwd_bindings()?,
        ))
    })?;
    assert_eq!(by_ext.map(|row| row.id), Some(id(50)));
    let cwds: Vec<_> = cwd_rows
        .iter()
        .filter_map(|row| row.cwd_prefix.clone())
        .collect();
    assert_eq!(cwds, ["C:/dev/a", "C:/dev/b"]);

    // A binding id narrows to that row, a cwd prefix to its bindings, and neither to all active.
    let (by_id, by_cwd, all) = store.read(|s| {
        Ok((
            s.active_bindings(Some(id(53)), Some("C:/dev/a"))?,
            s.active_bindings(None, Some("C:/dev/a"))?,
            s.active_bindings(None, None)?,
        ))
    })?;
    assert_eq!(by_id.iter().map(|row| row.id).collect::<Vec<_>>(), [id(53)]);
    assert_eq!(
        by_cwd.iter().map(|row| row.id).collect::<Vec<_>>(),
        [id(52)]
    );
    assert_eq!(all.len(), 3);

    let changed = store.write(|s| s.deactivate_bindings(&[id(52), id(53)]))?;
    assert_eq!(changed, 2);
    let remaining = store.read(|s| s.active_cwd_bindings())?;
    assert!(remaining.is_empty());
    Ok(())
}

#[test]
fn hook_and_tool_events_read_back_for_stats_and_dedupe() -> TestResult {
    let mut store = store()?;
    base(&mut store)?;
    put(&mut store, hook(60, "dk-1", "PreToolUse", at(1, 0)))?;
    put(&mut store, hook(61, "dk-2", "PostToolUse", at(2, 0)))?;
    put(&mut store, hook(62, "dk-3", "PostToolUse", at(3, 0)))?;
    put(
        &mut store,
        ToolEvent {
            id: id(63),
            hook_event_id: id(61),
            session_id: None,
            attempt_id: None,
            external_agent_id: None,
            phase: "success".into(),
            tool_name: "Bash".into(),
            tool_use_id: Some("tu-1".into()),
            duration_ms: Some(12),
            error_excerpt: None,
            summary: obj(json!({"command_head": "ls"})),
            occurred_at: at(2, 0),
        },
    )?;

    let (dup, count, last, by_name, tools) = store.read(|s| {
        Ok((
            s.hook_event_id_by_dedupe_key("dk-2")?,
            s.hook_event_count()?,
            s.hook_event_last_received()?,
            s.hook_event_counts_by_name()?,
            s.tool_events_for_hook(id(61))?,
        ))
    })?;
    assert_eq!(dup, Some(id(61)));
    assert_eq!(count, 3);
    assert_eq!(last, Some(at(3, 0)));
    let counts: Vec<_> = by_name
        .iter()
        .map(|row| (row.hook_event_name.as_str(), row.count))
        .collect();
    assert_eq!(counts, [("PostToolUse", 2), ("PreToolUse", 1)]);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].tool_name, "Bash");
    Ok(())
}

#[test]
fn stopped_runs_join_their_owning_task_and_subagent_selections_follow_python() -> TestResult {
    let mut store = store()?;
    base(&mut store)?;
    put(&mut store, session(20, "s-1", 1, Some(id(0))))?;
    let mut linked = subagent(30, 20, "agent-a", Some(at(5, 0)));
    linked.task_id = Some(id(10));
    linked.transcript_path = Some("a.jsonl".into());
    put(&mut store, linked)?;
    let mut open = subagent(31, 20, "agent-b", None);
    open.transcript_path = Some("b.jsonl".into());
    open.model = Some("claude".into());
    put(&mut store, open)?;
    let mut mentions = subagent(32, 20, "agent-c", Some(at(9, 0)));
    mentions.telemetry = Some(obj(json!({"trial_ids": ["trial-xyz"]})));
    put(&mut store, mentions)?;
    put(&mut store, subagent(33, 20, "agent-d", Some(at(7, 0))))?;

    let (stopped, owners, open_count, stopped_count, needing, limited, mention) =
        store.read(|s| {
            Ok((
                s.subagent_runs_by_stop(true)?,
                s.stopped_run_owners()?,
                s.subagent_run_count_by_stop(false)?,
                s.subagent_run_count_by_stop(true)?,
                s.subagent_runs_needing_model(None)?,
                s.subagent_runs_with_transcript(Some(1))?,
                s.subagent_runs_mentioning("trial-xyz")?,
            ))
        })?;
    let mut stopped_ids: Vec<Id> = stopped.iter().map(|row| row.id).collect();
    stopped_ids.sort_by_key(|id| id.to_string());
    let mut expected = vec![id(30), id(32), id(33)];
    expected.sort_by_key(|id| id.to_string());
    assert_eq!(stopped_ids, expected);
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].run_id, id(30));
    assert_eq!(owners[0].task_id, id(10));
    assert_eq!(owners[0].workspace_id, id(0));
    assert_eq!(owners[0].status, TaskStatus::Backlog);
    assert_eq!(open_count, 1);
    assert_eq!(stopped_count, 3);
    assert_eq!(
        needing.iter().map(|row| row.id).collect::<Vec<_>>(),
        [id(30)]
    );
    // Transcript rows are 30 and 31; `id` order picks one of them, and the limit keeps one.
    assert_eq!(limited.len(), 1);
    assert!(limited[0].id == id(30) || limited[0].id == id(31));
    assert_eq!(
        mention.iter().map(|row| row.id).collect::<Vec<_>>(),
        [id(32)]
    );
    Ok(())
}

#[test]
fn open_attempt_skips_attempts_already_bound_to_a_subagent() -> TestResult {
    let mut store = store()?;
    base(&mut store)?;
    let attempt = |n: u32, number: i64, status: AttemptStatus| AgentAttempt {
        id: id(n),
        workspace_id: id(0),
        task_id: id(10),
        number,
        status,
        provider: "claude_code".into(),
        adapter: "claude_code".into(),
        agent_id: None,
        model: None,
        session_id: None,
        parent_attempt_id: None,
        retry_of_attempt_id: None,
        rerun_of_attempt_id: None,
        created_by: "human:brice".into(),
        created_at: at(1, 0),
        queued_at: None,
        started_at: None,
        finished_at: None,
        queue_wait_ms: None,
        duration_ms: None,
        exit_reason: None,
        exit_code: None,
        error: None,
        summary: None,
        input_task_version: None,
        input_task_snapshot: obj(json!({})),
        prompt_artifact_id: None,
        config_snapshot: obj(json!({})),
        environment: obj(json!({})),
        baseline_manifest_id: None,
        outcome_manifest_id: None,
        usage: obj(json!({})),
        usage_source: None,
        cost_usd: None,
        cost_source: None,
        telemetry: obj(json!({})),
        modified_repository_ids: VectorSync::new_sync(),
        retries: 0,
        cancel_requested: false,
        updated_at: at(1, 0),
    };
    put(&mut store, attempt(70, 1, AttemptStatus::Running))?;
    put(&mut store, attempt(71, 2, AttemptStatus::Running))?;
    put(&mut store, attempt(72, 3, AttemptStatus::Succeeded))?;
    put(&mut store, session(20, "s-1", 1, Some(id(0))))?;
    let mut bound = subagent(30, 20, "agent-a", None);
    bound.attempt_id = Some(id(71));
    put(&mut store, bound)?;

    let open = store.read(|s| s.open_attempt_for_subagent(id(10)))?;
    assert_eq!(open.map(|row| row.id), Some(id(70)));
    Ok(())
}

#[test]
fn retention_selects_and_purges_payloads_and_telemetry_artifacts() -> TestResult {
    let mut store = store()?;
    base(&mut store)?;
    put(&mut store, hook(60, "dk-old", "PostToolUse", at(1, 0)))?;
    put(&mut store, hook(61, "dk-newer", "PostToolUse", at(9, 0)))?;
    put(&mut store, artifact(80, "telemetry", at(1, 0)))?;
    put(&mut store, artifact(81, "telemetry", at(9, 0)))?;
    put(&mut store, artifact(82, "keep", at(1, 0)))?;
    let cutoff = at(5, 0);

    let (due_hooks, due_artifacts) = store.read(|s| {
        Ok((
            s.hook_payloads_to_purge(cutoff)?,
            s.telemetry_artifacts_to_purge(cutoff)?,
        ))
    })?;
    assert_eq!(due_hooks, [id(60)]);
    assert_eq!(
        due_artifacts.iter().map(|row| row.id).collect::<Vec<_>>(),
        [id(80)]
    );

    let now = at(20, 0);
    let hooks_changed = store.write(|s| s.purge_hook_payloads(&due_hooks, now))?;
    let artifacts_changed = store.write(|s| s.mark_artifacts_purged(&[id(80)], now))?;
    assert_eq!(hooks_changed, 1);
    assert_eq!(artifacts_changed, 1);

    // The purged rows keep their metadata; the payload is gone and stamped.
    let (after_hooks, after_artifacts, count) = store.read(|s| {
        Ok((
            s.hook_payloads_to_purge(cutoff)?,
            s.telemetry_artifacts_to_purge(cutoff)?,
            s.hook_event_count()?,
        ))
    })?;
    assert!(after_hooks.is_empty());
    assert!(after_artifacts.is_empty());
    assert_eq!(count, 2);
    Ok(())
}
