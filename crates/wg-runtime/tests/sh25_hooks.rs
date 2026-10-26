//! Hook ingestion and spool replay checks (SH-25) against a real SQLite home: the hook payload
//! fixtures `tests/integration/test_claude_hooks.py` uses, run through `App::hook_ingest`, the spool
//! writer and `App::hook_replay_spool`.

use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use wg_model::entity::{
    AgentAttempt, AgentInternalTask, AgentSession, HookEvent, SubagentRun, Task, ToolEvent,
};
use wg_model::{RowKey, Write};
use wg_runtime::{App, Failure, IngestStatus, RequestContext, human_context, spool_record};
use wg_store::{Raw, Statement};
use wg_telemetry::ingest::{self, AttemptSpec};
use wg_types::{AttemptStatus, Id, parse_id};

type TestResult = Result<(), Box<dyn Error>>;

const MODEL: &str = "claude-haiku-4-5-20251001";

/// A Workgraph home with one workspace, and the context the checks act under.
struct Home {
    app: App,
    ctx: RequestContext,
    dir: PathBuf,
}

fn home(name: &str) -> Result<Home, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("sh25")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let mut app = App::open(Some(&dir))?;
    let ctx = human_context(Some("tester"))?;
    // The workspace the registry feature creates, and its default (`WorkspaceService.create`).
    let dump = object(json!({
        "slug": "main",
        "name": "Main",
        "description": "",
        "key_prefix": "WG",
    }));
    app.runtime.decide(
        &ctx,
        |session| session.registry_world(None),
        |slice, stamp| wg_registry::workspaces::create(slice, stamp, &dump),
    )?;
    let mut config = app.config()?;
    config.insert("default_workspace".to_owned(), json!("main"));
    app.save_config(&config)?;
    Ok(Home { app, ctx, dir })
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

/// `base(event, session, **kw)` of the Python tests.
fn base(event: &str, session: &str, extra: Value) -> Value {
    let mut map = object(json!({
        "session_id": session,
        "transcript_path": "/t.jsonl",
        "cwd": "/work",
        "hook_event_name": event,
    }));
    map.extend(object(extra));
    Value::Object(map)
}

fn record(payload: &Value, env: &Value, received_at: &str) -> Value {
    json!({"received_at": received_at, "env": env, "payload": payload})
}

const T0: &str = "2026-10-17T10:00:00+00:00";

impl Home {
    fn hook(&mut self, payload: &Value, env: &Value, at: &str) -> wg_runtime::IngestResult {
        match self.app.hook_ingest(&record(payload, env, at), "direct") {
            Ok(result) => result,
            Err(failure) => panic!("ingest failed: {failure}"),
        }
    }

    fn plain(&mut self, payload: &Value, at: &str) -> wg_runtime::IngestResult {
        self.hook(payload, &json!({}), at)
    }

    /// Creates a task and returns it.
    fn task(&mut self, title: &str) -> Result<Task, Box<dyn Error>> {
        wg_tasks_feature::service::task_create(
            &mut self.app,
            &self.ctx,
            Some("main"),
            &json!({"title": title}),
        )?;
        let tasks = self.read(|session| {
            let ws = session.rows(&sql("SELECT id FROM workspaces"))?;
            let id = ids(&ws)?.remove(0);
            session.tasks_in_workspace(id, None)
        })?;
        tasks
            .into_iter()
            .find(|task| task.title == title)
            .ok_or_else(|| "task not created".into())
    }

    fn read<T>(
        &mut self,
        body: impl FnOnce(&mut wg_store::Session<'_>) -> Result<T, wg_types::WgError>,
    ) -> Result<T, Box<dyn Error>> {
        Ok(self.app.runtime.store.read(body)?)
    }

    /// An attempt of `task` in `status` (`new_attempt` of the Python tests).
    fn attempt(&mut self, task: &Task, status: AttemptStatus) -> Result<Id, Box<dyn Error>> {
        let stamp = self.app.runtime.stamps.stamp(&self.ctx)?;
        let (attempt, created) = self.read(|session| {
            let number = ingest::next_number(&session.attempts_of_task(task.id, &[])?);
            let spec = AttemptSpec {
                adapter: "claude_code",
                agent_id: "tester".to_owned(),
                model: None,
                session_id: None,
                parent_attempt_id: None,
                started_at: None,
                telemetry: Map::new(),
            };
            Ok(ingest::new_attempt(
                &stamp,
                (stamp.id(0), stamp.id(1)),
                task,
                number,
                spec,
            ))
        })?;
        let attempt = AgentAttempt {
            status,
            finished_at: (status != AttemptStatus::Running).then_some(stamp.at),
            ..attempt
        };
        let id = attempt.id;
        self.app.runtime.store.write(|session| {
            session.apply_write(&Write::insert(attempt))?;
            session.append_event(&created).map(|_| ())
        })?;
        Ok(id)
    }

    fn session_row(&mut self, external: &str) -> Result<AgentSession, Box<dyn Error>> {
        self.read(|session| session.agent_session_by_external("claude_code", external))?
            .ok_or_else(|| "no session".into())
    }

    fn run_row(&mut self, agent: &str) -> Result<SubagentRun, Box<dyn Error>> {
        let rows = self.read(|session| {
            session.rows(&sql(&format!(
                "SELECT id FROM subagent_runs WHERE external_agent_id = '{agent}'"
            )))
        })?;
        let id = ids(&rows)?.remove(0);
        self.fetch(&RowKey::SubagentRun(id))
    }

    fn fetch<T: wg_store::TableRow>(&mut self, key: &RowKey) -> Result<T, Box<dyn Error>> {
        self.read(|session| session.fetch::<T>(key))?
            .ok_or_else(|| "row not found".into())
    }

    fn attempt_row(&mut self, id: Id) -> Result<AgentAttempt, Box<dyn Error>> {
        self.fetch(&RowKey::AgentAttempt(id))
    }

    fn count(&mut self, table: &str) -> Result<i64, Box<dyn Error>> {
        let rows =
            self.read(|session| session.rows(&sql(&format!("SELECT COUNT(*) FROM {table}"))))?;
        match rows.first().and_then(|row| row.first()) {
            Some(Raw::Int(n)) => Ok(*n),
            _ => Err("no count".into()),
        }
    }

    fn attempts_of(&mut self, task: &Task) -> Result<Vec<AgentAttempt>, Box<dyn Error>> {
        self.read(|session| session.attempts_of_task(task.id, &[]))
    }

    fn spool_dir(&self) -> PathBuf {
        self.dir.join("data").join("spool").join("claude-hooks")
    }
}

fn sql(text: &str) -> Statement {
    Statement {
        sql: text.to_owned(),
        params: Vec::new(),
    }
}

fn ids(rows: &[Vec<Raw>]) -> Result<Vec<Id>, wg_types::WgError> {
    rows.iter()
        .map(|row| match row.first() {
            Some(Raw::Text(text)) => parse_id(text),
            _ => Err(wg_types::WgError::new(
                wg_types::ErrorCode::WorkgraphError,
                "not an id",
            )),
        })
        .collect()
}

/// `write_subagent_transcript`.
fn write_transcript(path: &Path, task_key: Option<&str>, output: u64, quoted: Option<&str>) {
    let mut prompt = match task_key {
        Some(key) => format!("Task-ID: {key}\nDo the work."),
        None => "Do the work.".to_owned(),
    };
    if let Some(key) = quoted {
        prompt.push_str(&format!("\n\n## Task\nOld change.\n\nTask-ID: {key}\n"));
    }
    let usage = |out: u64| {
        json!({
            "input_tokens": 3, "cache_creation_input_tokens": 10,
            "cache_read_input_tokens": 20, "output_tokens": out,
        })
    };
    let records = [
        json!({"type": "user", "timestamp": "2026-10-14T08:54:30.000Z",
               "message": {"role": "user", "content": prompt}}),
        json!({"type": "assistant", "timestamp": "2026-10-14T08:54:31.000Z",
               "message": {"id": "msg_1", "model": MODEL,
                           "content": [{"type": "tool_use", "id": "tu_1", "name": "Bash", "input": {}}],
                           "usage": usage(3)}}),
        json!({"type": "assistant", "timestamp": "2026-10-14T08:54:32.000Z",
               "message": {"id": "msg_1", "model": MODEL, "content": [], "usage": usage(output)}}),
    ];
    let text: String = records.iter().map(|r| format!("{r}\n")).collect();
    assert!(std::fs::write(path, text).is_ok());
}

/// Starts and stops a subagent in a session with no Workgraph launch context.
fn stop_untracked(home: &mut Home, session: &str, agent: &str, transcript: &Path, at: &str) {
    home.plain(
        &base("SessionStart", session, json!({"source": "startup"})),
        at,
    );
    home.plain(
        &base(
            "SubagentStart",
            session,
            json!({"agent_id": agent, "agent_type": "general-purpose"}),
        ),
        at,
    );
    home.plain(
        &base(
            "SubagentStop",
            session,
            json!({"agent_id": agent, "agent_type": "general-purpose",
                   "agent_transcript_path": transcript.to_string_lossy()}),
        ),
        "2026-10-17T10:00:02+00:00",
    );
}

fn at_second(n: u32) -> String {
    format!("2026-10-17T10:00:{n:02}+00:00")
}

// covers: P01
#[test]
fn session_correlation_tools_subagents_and_internal_tasks() -> TestResult {
    let mut h = home("correlation")?;
    let task = h.task("Hooked")?;
    let attempt_id = h.attempt(&task, AttemptStatus::Running)?;
    let env = json!({"attempt_id": attempt_id.to_string(), "task_id": task.id.to_string()});
    let key = task.key.to_string();

    let started = h.hook(
        &base(
            "SessionStart",
            "sess-1",
            json!({"source": "startup", "model": "claude-x"}),
        ),
        &env,
        T0,
    );
    assert_eq!(started.status, IngestStatus::Ingested);
    let pre = h.hook(
        &base(
            "PreToolUse",
            "sess-1",
            json!({"tool_name": "Bash", "tool_use_id": "t1", "tool_input":
                   {"command": "export TOKEN=sk-ant-abcdefghijklmnopqrstuvwxyz && pytest"}}),
        ),
        &env,
        &at_second(1),
    );
    h.hook(
        &base(
            "PostToolUse",
            "sess-1",
            json!({"tool_name": "Bash", "tool_use_id": "t1", "tool_input": {"command": "pytest"},
                   "tool_response": {"stdout": "ok"}, "duration_ms": 1200}),
        ),
        &env,
        &at_second(2),
    );
    h.hook(
        &base(
            "PostToolUseFailure",
            "sess-1",
            json!({"tool_name": "Edit", "tool_use_id": "t2", "tool_input": {"file_path": "/work/a.py"},
                   "error": "Exit code 1 password=hunter2", "duration_ms": 5}),
        ),
        &env,
        &at_second(3),
    );
    h.hook(
        &base(
            "SubagentStart",
            "sess-1",
            json!({"agent_id": "agent-1", "agent_type": "Explore"}),
        ),
        &env,
        &at_second(4),
    );
    h.hook(
        &base(
            "SubagentStop",
            "sess-1",
            json!({"agent_id": "agent-1", "agent_type": "Explore",
                   "last_assistant_message": "Found 3 issues", "stop_hook_active": false}),
        ),
        &env,
        &at_second(5),
    );
    let subject = format!("Implement part of {key}");
    h.hook(
        &base(
            "TaskCreated",
            "sess-1",
            json!({"task_id": "task-001", "task_subject": subject}),
        ),
        &env,
        &at_second(6),
    );
    h.hook(
        &base(
            "TaskCompleted",
            "sess-1",
            json!({"task_id": "task-001", "task_subject": subject}),
        ),
        &env,
        &at_second(7),
    );
    h.hook(
        &base(
            "Stop",
            "sess-1",
            json!({"last_assistant_message": "done", "stop_hook_active": false}),
        ),
        &env,
        &at_second(8),
    );
    h.hook(
        &base("SessionEnd", "sess-1", json!({"reason": "other"})),
        &env,
        &at_second(9),
    );

    let session = h.session_row("sess-1")?;
    assert_eq!(session.binding_source, "workgraph_launched");
    assert_eq!(session.attempt_id, Some(attempt_id));
    assert_eq!(session.task_id, Some(task.id));
    assert_eq!((session.tool_calls, session.tool_failures), (2, 1));
    assert_eq!(session.end_reason.as_deref(), Some("other"));
    assert_eq!(session.model.as_deref(), Some("claude-x"));

    let tool_ids = h.read(|s| s.rows(&sql("SELECT id FROM tool_events ORDER BY occurred_at")))?;
    let mut tools = Vec::new();
    for id in ids(&tool_ids)? {
        tools.push(h.fetch::<ToolEvent>(&RowKey::ToolEvent(id))?);
    }
    let phases: Vec<&str> = tools.iter().map(|t| t.phase.as_str()).collect();
    assert_eq!(phases, ["pre", "success", "failure"]);
    assert_eq!(tools[2].summary["file_path"], "/work/a.py");
    assert!(
        !tools[2]
            .error_excerpt
            .clone()
            .unwrap_or_default()
            .contains("hunter2")
    );
    assert_eq!(tools[1].duration_ms, Some(1200));
    assert_eq!(tools[0].summary["command_head"], "export");

    let hook_row: HookEvent = h.fetch(&RowKey::HookEvent(pre.hook_event_id.ok_or("no hook")?))?;
    let stored = Value::Object(hook_row.payload.unwrap_or_default()).to_string();
    assert!(
        !stored.contains("sk-ant"),
        "tool payloads are summarised, not stored"
    );

    let run = h.run_row("agent-1")?;
    assert_eq!(run.agent_type.as_deref(), Some("Explore"));
    assert!(run.stopped_at.is_some());
    assert_eq!(run.last_message_excerpt.as_deref(), Some("Found 3 issues"));
    let child = h.attempt_row(run.attempt_id.ok_or("no child attempt")?)?;
    assert_eq!(child.parent_attempt_id, Some(attempt_id));
    assert_eq!(child.status, AttemptStatus::Succeeded);
    assert_eq!(child.exit_reason.as_deref(), Some("subagent_stopped"));
    assert_eq!(child.summary.as_deref(), Some("Found 3 issues"));

    let internal_ids = h.read(|s| s.rows(&sql("SELECT id FROM agent_internal_tasks")))?;
    let internal: AgentInternalTask =
        h.fetch(&RowKey::AgentInternalTask(ids(&internal_ids)?.remove(0)))?;
    assert_eq!(internal.status, "completed");
    assert_eq!(internal.task_id, Some(task.id));

    let parent = h.attempt_row(attempt_id)?;
    assert_eq!(parent.telemetry["hook_tool_calls"], 2);
    assert_eq!(parent.telemetry["hook_tool_failures"], 1);
    assert_eq!(parent.telemetry["hook_subagents"], 1);

    let events = h.read(|s| {
        s.rows(&sql(
            "SELECT event_type FROM events WHERE event_type LIKE 'agent_session.%' ORDER BY seq",
        ))
    })?;
    let names: Vec<String> = events
        .iter()
        .filter_map(|row| match row.first() {
            Some(Raw::Text(text)) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        names,
        [
            "agent_session.started",
            "agent_session.subagent_started",
            "agent_session.subagent_stopped",
            "agent_session.internal_task_created",
            "agent_session.internal_task_completed",
            "agent_session.ended",
        ]
    );
    Ok(())
}

// covers: P01
#[test]
fn ingestion_is_idempotent() -> TestResult {
    let mut h = home("idempotent")?;
    let payload = base("SessionStart", "dup", json!({"source": "startup"}));
    assert_eq!(h.plain(&payload, T0).status, IngestStatus::Ingested);
    assert_eq!(h.plain(&payload, T0).status, IngestStatus::Duplicate);
    assert_eq!(h.count("hook_events")?, 1);
    // The same payload at another time is a different delivery.
    assert_eq!(
        h.plain(&payload, &at_second(1)).status,
        IngestStatus::Ingested
    );
    assert_eq!(h.count("hook_events")?, 2);
    assert_eq!(h.count("agent_sessions")?, 1);
    Ok(())
}

// covers: P01
#[test]
fn unbound_session_is_bound_by_cwd_prefix() -> TestResult {
    let mut h = home("bind-cwd")?;
    let task = h.task("External session")?;
    let target = h.dir.join("project");
    std::fs::create_dir_all(&target)?;
    let ctx = h.ctx.clone();
    let bound = h.app.hook_bind(
        &ctx,
        Some("main"),
        &task.key.to_string(),
        None,
        target.to_str(),
    )?;
    assert_eq!(bound["task"], task.key.to_string());

    let mut payload = base("SessionStart", "ext-1", json!({"source": "startup"}));
    payload["cwd"] = json!(format!("{}/sub", target.display()));
    h.plain(&payload, T0);
    let session = h.session_row("ext-1")?;
    assert_eq!(session.binding_source, "explicit_binding");
    assert_eq!(session.task_id, Some(task.id));

    let mut elsewhere = base("SessionStart", "ext-2", json!({}));
    elsewhere["cwd"] = json!("/elsewhere");
    h.plain(&elsewhere, T0);
    assert_eq!(h.session_row("ext-2")?.binding_source, "unbound");

    let sessions = h.app.hook_sessions(None, 50)?;
    assert_eq!(sessions.len(), 2);

    // A later binding by session id adopts the unbound session at its next hook.
    let ctx = h.ctx.clone();
    h.app.hook_bind(
        &ctx,
        Some("main"),
        &task.key.to_string(),
        Some("ext-2"),
        None,
    )?;
    assert_eq!(h.session_row("ext-2")?.binding_source, "explicit_binding");
    assert_eq!(h.app.hook_unbind(None, None)?, 2);
    Ok(())
}

// covers: F09
#[test]
fn spooled_records_replay_once() -> TestResult {
    let mut h = home("spool")?;
    let rec = record(&base("SessionStart", "sp", json!({})), &json!({}), T0);
    let failure = Failure::new("RuntimeError", "database is locked");
    let file = spool_record(Some(&h.dir), &rec, &failure)?;
    assert_eq!(file.parent(), Some(h.spool_dir().as_path()));
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("name")?
        .to_owned();
    assert!(
        name.starts_with("20261017T100000+0000-") && name.ends_with(".json"),
        "{name}"
    );
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
    assert!(
        stored["spool_reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("RuntimeError"))
    );
    assert_eq!(stored["received_at"], T0);

    let counts = h.app.hook_replay_spool(500)?;
    assert_eq!(
        (counts.ingested, counts.duplicate, counts.failed),
        (1, 0, 0)
    );
    assert!(!file.exists());
    assert_eq!(h.session_row("sp")?.external_session_id, "sp");
    let source = h.read(|s| s.rows(&sql("SELECT source FROM hook_events")))?;
    assert_eq!(source[0][0], Raw::Text("spool".to_owned()));
    let again = h.app.hook_replay_spool(500)?;
    assert_eq!((again.ingested, again.duplicate, again.failed), (0, 0, 0));

    // A record that was ingested directly and spooled too is a duplicate.
    h.plain(&base("SessionStart", "sp2", json!({})), T0);
    let twin = record(&base("SessionStart", "sp2", json!({})), &json!({}), T0);
    spool_record(Some(&h.dir), &twin, &failure)?;
    let counts = h.app.hook_replay_spool(500)?;
    // The spooled copy was redacted, which can change its dedupe key; either way the file goes.
    assert_eq!(counts.ingested + counts.duplicate, 1);
    assert!(std::fs::read_dir(h.spool_dir())?.next().is_none());
    Ok(())
}

// covers: F09
#[test]
fn a_record_failing_five_replays_is_set_aside() -> TestResult {
    let mut h = home("spool-failed")?;
    let bad = record(&json!({"hook_event_name": 5}), &json!({}), T0);
    let file = spool_record(Some(&h.dir), &bad, &Failure::new("ValueError", "bad"))?;
    for round in 1..=4 {
        let counts = h.app.hook_replay_spool(500)?;
        assert_eq!(counts.failed, 1, "round {round}");
        assert!(file.exists());
        let kept: Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
        assert_eq!(kept["replay_attempts"], round);
        assert!(
            kept["last_replay_error"]
                .as_str()
                .is_some_and(|e| e.starts_with("ValidationError"))
        );
    }
    let counts = h.app.hook_replay_spool(500)?;
    assert_eq!(counts.failed, 1);
    assert!(!file.exists());
    let failed = file.with_extension("failed");
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(&failed)?)?;
    assert_eq!(kept["replay_attempts"], 5);
    let stats = h.app.hook_stats()?;
    assert_eq!(
        (stats["spooled"].clone(), stats["spool_failed"].clone()),
        (json!(0), json!(1))
    );

    // A file that is not a JSON object is set aside at once.
    std::fs::write(h.spool_dir().join("garbage.json"), "{not json")?;
    let counts = h.app.hook_replay_spool(500)?;
    assert_eq!(counts.failed, 1);
    assert!(h.spool_dir().join("garbage.failed").exists());
    Ok(())
}

// covers: P01
#[test]
fn invalid_payloads_are_wrapped_and_unknown_keys_kept() -> TestResult {
    let mut h = home("invalid")?;
    let wrapped = h.plain(&json!("not an object"), T0);
    let row: HookEvent = h.fetch(&RowKey::HookEvent(wrapped.hook_event_id.ok_or("no hook")?))?;
    assert_eq!(row.hook_event_name, "Invalid");
    assert_eq!(
        row.payload.as_ref().map(|p| p["original"].clone()),
        Some(json!("not an object"))
    );
    assert_eq!(wrapped.session_id, None);

    let newer = h.plain(
        &base(
            "SessionStart",
            "v2",
            json!({"effort": "high", "flavour": 3}),
        ),
        T0,
    );
    let row: HookEvent = h.fetch(&RowKey::HookEvent(newer.hook_event_id.ok_or("no hook")?))?;
    let payload = row.payload.unwrap_or_default();
    assert_eq!(payload["flavour"], 3);
    assert_eq!(
        payload["_workgraph"],
        json!({"schema_version": 2, "source": "direct"})
    );

    let failure = match h.app.hook_ingest(
        &record(&json!({"hook_event_name": 5}), &json!({}), T0),
        "direct",
    ) {
        Ok(_) => return Err("accepted an invalid hook".into()),
        Err(failure) => failure,
    };
    assert_eq!(failure.type_name, "ValidationError");
    Ok(())
}

// covers: P01
#[test]
fn a_duration_beyond_the_database_integer_fails_the_whole_ingestion() -> TestResult {
    let mut h = home("overflow")?;
    let payload = base(
        "PostToolUse",
        "big",
        json!({"tool_name": "Bash", "tool_use_id": "t", "duration_ms": 123_456_789_012_345_678_901_u128}),
    );
    let failure = match h
        .app
        .hook_ingest(&record(&payload, &json!({}), T0), "direct")
    {
        Ok(_) => return Err("ingested an overflowing duration".into()),
        Err(failure) => failure,
    };
    assert_eq!(failure.type_name, "OverflowError");
    // Python rolls the transaction back: no hook, no session.
    assert_eq!(
        (h.count("hook_events")?, h.count("agent_sessions")?),
        (0, 0)
    );
    // The same duration on an event that records no tool call is accepted.
    let quiet = base(
        "Stop",
        "big",
        json!({"duration_ms": 123_456_789_012_345_678_901_u128}),
    );
    assert_eq!(h.plain(&quiet, T0).status, IngestStatus::Ingested);
    Ok(())
}

// covers: P01
#[test]
fn only_the_first_task_marker_links_the_run() -> TestResult {
    let mut h = home("first-marker")?;
    let task = h.task("Brief's own task")?;
    let quoted = h.task("Task named by a quoted commit message")?;
    let transcript = h.dir.join("agent-q1.jsonl");
    write_transcript(
        &transcript,
        Some(&task.key.to_string()),
        40,
        Some(&quoted.key.to_string()),
    );
    h.plain(
        &base("SessionStart", "sess-quoted", json!({"source": "startup"})),
        T0,
    );
    h.plain(
        &base(
            "SubagentStop",
            "sess-quoted",
            json!({"agent_id": "agent-q1", "agent_type": "general-purpose",
                   "agent_transcript_path": transcript.to_string_lossy()}),
        ),
        &at_second(2),
    );
    let run = h.run_row("agent-q1")?;
    assert_eq!(run.task_id, Some(task.id));
    let telemetry = run.telemetry.unwrap_or_default();
    assert_eq!(
        telemetry["task_keys"],
        json!([task.key.to_string(), quoted.key.to_string()])
    );
    Ok(())
}

// covers: P01
#[test]
fn unbound_subagent_transcript_creates_a_linked_top_level_attempt() -> TestResult {
    let mut h = home("untracked")?;
    let task = h.task("Transcript target")?;
    let transcript = h.dir.join("agent-u1.jsonl");
    write_transcript(&transcript, Some(&task.key.to_string()), 40, None);
    h.plain(
        &base(
            "SessionStart",
            "sess-untracked",
            json!({"source": "startup"}),
        ),
        T0,
    );
    h.plain(
        &base(
            "SubagentStart",
            "sess-untracked",
            json!({"agent_id": "agent-u1", "agent_type": "general-purpose"}),
        ),
        &at_second(1),
    );
    h.plain(
        &base(
            "SubagentStop",
            "sess-untracked",
            json!({"agent_id": "agent-u1", "agent_type": "general-purpose",
                   "agent_transcript_path": transcript.to_string_lossy(),
                   "last_assistant_message": "done"}),
        ),
        &at_second(2),
    );
    let run = h.run_row("agent-u1")?;
    assert_eq!(run.task_id, Some(task.id));
    assert_eq!(run.model.as_deref(), Some(MODEL));
    assert_eq!(
        Value::Object(run.usage.clone().unwrap_or_default()),
        json!({"input_tokens": 3, "output_tokens": 40,
               "cache_creation_input_tokens": 10, "cache_read_input_tokens": 20})
    );
    let attempt = h.attempt_row(run.attempt_id.ok_or("no attempt")?)?;
    assert_eq!(attempt.task_id, task.id);
    assert_eq!(attempt.parent_attempt_id, None);
    assert_eq!(attempt.adapter, "claude_code_subagent");
    assert_eq!(
        attempt.agent_id.as_deref(),
        Some("subagent:general-purpose")
    );
    assert_eq!(attempt.model.as_deref(), Some(MODEL));
    assert_eq!(attempt.usage["output_tokens"], 40);
    assert_eq!(
        attempt.usage_source.as_deref(),
        Some("claude_code_transcript")
    );
    assert_eq!(attempt.session_id.as_deref(), Some("sess-untracked"));
    assert_eq!(attempt.status, AttemptStatus::Succeeded);
    assert_eq!(attempt.telemetry["tool_uses"], json!({"Bash": 1}));
    assert_eq!(attempt.summary.as_deref(), Some("done"));
    Ok(())
}

// covers: P01
#[test]
fn subagent_transcript_binds_to_the_running_orchestrator_attempt() -> TestResult {
    let mut h = home("binds")?;
    let task = h.task("Hooked")?;
    let attempt_id = h.attempt(&task, AttemptStatus::Running)?;
    let transcript = h.dir.join("agent-o1.jsonl");
    write_transcript(&transcript, Some(&task.key.to_string()), 25, None);
    stop_untracked(&mut h, "sess-orch", "agent-o1", &transcript, T0);
    let run = h.run_row("agent-o1")?;
    assert_eq!(
        (run.attempt_id, run.task_id),
        (Some(attempt_id), Some(task.id))
    );
    assert_eq!(
        h.attempts_of(&task)?.len(),
        1,
        "no second attempt was created"
    );
    let attempt = h.attempt_row(attempt_id)?;
    assert_eq!(attempt.status, AttemptStatus::Running);
    assert!(attempt.finished_at.is_none());
    assert_eq!(attempt.model.as_deref(), Some(MODEL));
    assert_eq!(attempt.usage["output_tokens"], 25);
    assert_eq!(
        attempt.usage_source.as_deref(),
        Some("claude_code_transcript")
    );
    assert_eq!(attempt.telemetry["tool_uses"], json!({"Bash": 1}));
    assert_eq!(attempt.adapter, "claude_code");
    let bound = h.read(|s| {
        s.rows(&sql(
            "SELECT data FROM events WHERE event_type = 'attempt.subagent_bound'",
        ))
    })?;
    assert_eq!(bound.len(), 1);
    Ok(())
}

// covers: P01
#[test]
fn a_repeated_subagent_stop_leaves_the_bound_attempt_open() -> TestResult {
    let mut h = home("repeat-stop")?;
    let task = h.task("Hooked")?;
    let attempt_id = h.attempt(&task, AttemptStatus::Running)?;
    let transcript = h.dir.join("agent-o2.jsonl");
    write_transcript(&transcript, Some(&task.key.to_string()), 40, None);
    stop_untracked(&mut h, "sess-repeat", "agent-o2", &transcript, T0);
    h.plain(
        &base(
            "SubagentStop",
            "sess-repeat",
            json!({"agent_id": "agent-o2", "agent_type": "general-purpose",
                   "agent_transcript_path": transcript.to_string_lossy()}),
        ),
        &at_second(5),
    );
    let attempt = h.attempt_row(attempt_id)?;
    assert_eq!(attempt.status, AttemptStatus::Running);
    assert!(attempt.finished_at.is_none());
    Ok(())
}

// covers: P01
#[test]
fn a_transcript_creates_an_attempt_when_only_finished_attempts_exist() -> TestResult {
    let mut h = home("finished-only")?;
    let task = h.task("Already finished")?;
    let done = h.attempt(&task, AttemptStatus::Succeeded)?;
    let transcript = h.dir.join("agent-f1.jsonl");
    write_transcript(&transcript, Some(&task.key.to_string()), 40, None);
    stop_untracked(&mut h, "sess-finished", "agent-f1", &transcript, T0);
    let run = h.run_row("agent-f1")?;
    assert_ne!(run.attempt_id, Some(done));
    let created = h.attempt_row(run.attempt_id.ok_or("no attempt")?)?;
    assert_eq!(created.number, 2);
    assert_eq!(created.adapter, "claude_code_subagent");
    assert_eq!(created.status, AttemptStatus::Succeeded);
    assert_eq!(h.attempt_row(done)?.status, AttemptStatus::Succeeded);
    Ok(())
}

// covers: P01
#[test]
fn an_attempt_already_bound_to_a_subagent_is_not_reused() -> TestResult {
    let mut h = home("not-reused")?;
    let task = h.task("Hooked")?;
    let attempt_id = h.attempt(&task, AttemptStatus::Running)?;
    let first = h.dir.join("agent-r1.jsonl");
    let second = h.dir.join("agent-r2.jsonl");
    write_transcript(&first, Some(&task.key.to_string()), 40, None);
    write_transcript(&second, Some(&task.key.to_string()), 40, None);
    stop_untracked(&mut h, "sess-r1", "agent-r1", &first, T0);
    stop_untracked(
        &mut h,
        "sess-r2",
        "agent-r2",
        &second,
        "2026-10-17T10:01:00+00:00",
    );
    let bound = h.run_row("agent-r1")?;
    let other = h.run_row("agent-r2")?;
    assert_eq!(bound.attempt_id, Some(attempt_id));
    assert_ne!(other.attempt_id, Some(attempt_id));
    let created = h.attempt_row(other.attempt_id.ok_or("no attempt")?)?;
    assert_eq!(
        (created.number, created.adapter.as_str()),
        (2, "claude_code_subagent")
    );
    assert_eq!(h.attempt_row(attempt_id)?.status, AttemptStatus::Running);
    Ok(())
}

// covers: P01
#[test]
fn a_child_attempt_gets_the_transcript_model_and_usage() -> TestResult {
    let mut h = home("child")?;
    let task = h.task("Hooked")?;
    let attempt_id = h.attempt(&task, AttemptStatus::Running)?;
    let env = json!({"attempt_id": attempt_id.to_string(), "task_id": task.id.to_string()});
    let transcript = h.dir.join("agent-b1.jsonl");
    write_transcript(&transcript, None, 12, None);
    h.hook(
        &base("SessionStart", "sess-child", json!({"source": "startup"})),
        &env,
        T0,
    );
    h.hook(
        &base(
            "SubagentStart",
            "sess-child",
            json!({"agent_id": "agent-b1", "agent_type": "Explore"}),
        ),
        &env,
        &at_second(1),
    );
    h.hook(
        &base(
            "SubagentStop",
            "sess-child",
            json!({"agent_id": "agent-b1", "agent_type": "Explore",
                   "agent_transcript_path": transcript.to_string_lossy()}),
        ),
        &env,
        &at_second(2),
    );
    let run = h.run_row("agent-b1")?;
    let child = h.attempt_row(run.attempt_id.ok_or("no child")?)?;
    assert_eq!(child.parent_attempt_id, Some(attempt_id));
    assert_eq!(child.status, AttemptStatus::Succeeded);
    assert_eq!(child.model.as_deref(), Some(MODEL));
    assert_eq!(child.usage["output_tokens"], 12);
    assert_eq!(
        child.usage_source.as_deref(),
        Some("claude_code_transcript")
    );
    assert_eq!(run.task_id, None, "the transcript names no task");
    Ok(())
}

// covers: P01
#[test]
fn backfill_links_late_transcripts_idempotently() -> TestResult {
    let mut h = home("backfill")?;
    let task = h.task("Backfill target")?;
    let transcript = h.dir.join("agent-late.jsonl");
    h.plain(
        &base("SessionStart", "sess-late", json!({"source": "startup"})),
        T0,
    );
    h.plain(
        &base(
            "SubagentStart",
            "sess-late",
            json!({"agent_id": "agent-late", "agent_type": "general-purpose"}),
        ),
        &at_second(1),
    );
    h.plain(
        &base(
            "SubagentStop",
            "sess-late",
            json!({"agent_id": "agent-late", "agent_type": "general-purpose",
                   "agent_transcript_path": transcript.to_string_lossy()}),
        ),
        &at_second(2),
    );
    let ctx = h.ctx.clone();
    assert_eq!(
        h.app.hook_backfill_transcripts(&ctx, None)?,
        json!({"parsed": 0, "missing": 1, "linked": 0, "attempts_created": 0})
    );
    write_transcript(&transcript, Some(&task.key.to_string()), 40, None);
    assert_eq!(
        h.app.hook_backfill_transcripts(&ctx, None)?,
        json!({"parsed": 1, "missing": 0, "linked": 1, "attempts_created": 1})
    );
    assert_eq!(
        h.app.hook_backfill_transcripts(&ctx, None)?,
        json!({"parsed": 0, "missing": 0, "linked": 0, "attempts_created": 0})
    );
    let attempts = h.attempts_of(&task)?;
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0].usage_source.as_deref(),
        Some("claude_code_transcript")
    );

    // Everything is parsed already, so the time backfill finds the breakdown in the telemetry.
    let timed = h.app.hook_backfill_time(&ctx, None)?;
    assert_eq!(timed["skipped"], 1);
    Ok(())
}

// covers: P01
#[test]
fn stats_count_the_ledger() -> TestResult {
    let mut h = home("stats")?;
    h.plain(&base("SessionStart", "s", json!({})), T0);
    h.plain(&base("Stop", "s", json!({})), &at_second(1));
    h.plain(&base("Stop", "s", json!({})), &at_second(2));
    let stats = h.app.hook_stats()?;
    assert_eq!(stats["hook_events"], 3);
    assert_eq!(stats["by_event"], json!({"SessionStart": 1, "Stop": 2}));
    assert_eq!(stats["spooled"], 0);
    Ok(())
}
