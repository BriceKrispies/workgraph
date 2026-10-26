//! wg-runtime checks (SH-23) against real SQLite files, real git and real processes: the
//! load-decide-apply cycle, the effect interpreter, the flow driver, JSON logging and the
//! application context.
//!
//! Databases are built from `contracts/schema-0011.sql` (the migration runner is not part of the
//! runtime), in a directory under the target's test scratch space.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rpds::VectorSync;
use serde_json::Value;
use wg_model::entity::Workspace;
use wg_model::event::WorkspaceCreated;
use wg_model::{
    Decision, Effect, EffectOutcome, EffectResult, EventDraft, EventPayload, FsOp, GitOp, JobWake,
    ProcessSpec, Stamp, StreamEvent, Write,
};
use wg_runtime::{
    App, Flow, Interpreter, Level, LogSettings, RequestContext, RotatingFile, Runtime, StampSource,
    json_layer,
};
use wg_store::{EventQuery, Location, Raw, Statement, Store};
use wg_types::{ErrorCode, Id, Timestamp, WgError};

type TestResult = Result<(), Box<dyn Error>>;

/// A fresh scratch directory for one test.
fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("sh23")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
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

fn create_schema(store: &mut Store) -> TestResult {
    let ddl = schema()?;
    store.write(|session| session.execute_batch(&ddl))?;
    Ok(())
}

/// Stamps at a fixed instant with entropy that changes per decision, so runs are reproducible.
struct FixedStamps {
    next: u8,
}

impl StampSource for FixedStamps {
    fn stamp(&mut self, context: &RequestContext) -> Result<Stamp, WgError> {
        self.next = self.next.wrapping_add(1);
        let seed = self.next;
        Ok(context.stamp(at(), core::array::from_fn(|i| seed.wrapping_add(i as u8))))
    }
}

fn at() -> Timestamp {
    Timestamp::from_components(2026, 10, 25, 12, 0, 5, 123_456).unwrap_or_else(|e| panic!("{e}"))
}

fn runtime(dir: &Path) -> Result<Runtime, Box<dyn Error>> {
    let mut store = Store::open(&Location::File(dir.join("workgraph.db")))?;
    create_schema(&mut store)?;
    Ok(Runtime {
        store,
        stamps: Box::new(FixedStamps { next: 0 }),
        effects: Interpreter::default(),
    })
}

fn context() -> Result<RequestContext, WgError> {
    RequestContext::human(Some("tester"), None, wg_model::derive_id(&[9; 32], 0))
}

fn workspace(id: Id, slug: &str, stamp: &Stamp) -> Workspace {
    Workspace {
        id,
        slug: slug.to_owned(),
        name: slug.to_uppercase(),
        description: String::new(),
        key_prefix: "WG".to_owned(),
        next_task_number: 1,
        settings: Default::default(),
        archived_at: None,
        created_at: stamp.at,
        updated_at: stamp.at,
    }
}

/// `workspace create <slug>` as a decision, with extra effects.
fn create_workspace(
    slug: &str,
    stamp: &Stamp,
    effects: &[Effect],
) -> Result<Decision<Id>, WgError> {
    let ([workspace_id, event_id], _) = stamp.ids().take::<2>();
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(workspace_id),
        workspace_id,
        EventPayload::WorkspaceCreated(WorkspaceCreated {
            slug: slug.to_owned(),
            name: slug.to_uppercase(),
            key_prefix: "WG".to_owned(),
        }),
    );
    Ok(effects.iter().cloned().fold(
        Decision::new(workspace_id)
            .write(Write::insert(workspace(workspace_id, slug, stamp)))
            .event(event),
        Decision::effect,
    ))
}

fn count(session: &mut wg_store::Session<'_>, table: &str) -> Result<i64, WgError> {
    match session
        .rows(&Statement {
            sql: format!("SELECT COUNT(*) FROM {table}"),
            params: Vec::new(),
        })?
        .first()
        .and_then(|row| row.first())
    {
        Some(Raw::Int(n)) => Ok(*n),
        _ => Err(WgError::new(ErrorCode::WorkgraphError, "no count")),
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn decision_cycle_applies_then_performs_effects() -> TestResult {
    let dir = scratch("cycle")?;
    let mut runtime = runtime(&dir)?;
    let marker = dir.join("out").join("marker.txt");
    let effects = [
        Effect::Fs(FsOp::WriteAtomic {
            path: text(&marker),
            contents: "written after commit".to_owned(),
        }),
        Effect::Wake(JobWake {
            workspace_id: None,
            kind: "git.sync".to_owned(),
        }),
    ];
    let ctx = context()?;
    let outcome = runtime.decide(
        &ctx,
        |session| count(session, "workspaces"),
        |existing, stamp| {
            assert_eq!(*existing, 0);
            create_workspace("ops", stamp, &effects)
        },
    )?;
    assert_eq!(outcome.applied.event_seqs, [1]);
    assert_eq!(outcome.stamp.at, at());
    assert_eq!(outcome.effects.len(), 2);
    assert_eq!(outcome.effects[0].outcome, EffectOutcome::Fs(Ok(())));
    assert_eq!(outcome.effects[1].outcome, EffectOutcome::Done);
    assert_eq!(std::fs::read_to_string(&marker)?, "written after commit");
    assert_eq!(runtime.effects.bus.wakes(), 1);

    let events = runtime
        .store
        .read(|s| s.query_events(&EventQuery::default()))?;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "workspace.created");
    assert_eq!(events[0].actor, "human:tester");
    assert_eq!(events[0].correlation_id, ctx.correlation);
    assert_eq!(events[0].entity_id, outcome.reply);
    Ok(())
}

#[test]
fn failed_decision_rolls_back_and_performs_no_effect() -> TestResult {
    let dir = scratch("rollback")?;
    let mut runtime = runtime(&dir)?;
    let ctx = context()?;
    runtime.decide(
        &ctx,
        |_| Ok(()),
        |(), stamp| create_workspace("ops", stamp, &[]),
    )?;
    let marker = dir.join("never.txt");
    let effects = [Effect::Fs(FsOp::WriteAtomic {
        path: text(&marker),
        contents: "x".to_owned(),
    })];
    // The same slug again: the insert fails on the unique slug, after the event was drafted.
    let err = runtime
        .decide(
            &ctx,
            |_| Ok(()),
            |(), stamp| create_workspace("ops", stamp, &effects),
        )
        .err()
        .ok_or("no error")?;
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(!marker.exists());
    assert_eq!(runtime.store.read(|s| count(s, "workspaces"))?, 1);
    assert_eq!(runtime.store.read(|s| s.latest_seq())?, 1);

    // A decision that refuses writes nothing either.
    let refused = runtime.decide(
        &ctx,
        |s| count(s, "workspaces"),
        |_, _| -> Result<Decision<()>, WgError> {
            Err(WgError::new(ErrorCode::InvalidTransition, "no"))
        },
    );
    assert_eq!(
        refused.err().map(|e| e.code),
        Some(ErrorCode::InvalidTransition)
    );
    let total = runtime.query(|s| count(s, "events"), |n| Ok(*n))?;
    assert_eq!(total, 1);
    Ok(())
}

#[test]
fn git_and_process_effects_run_real_processes() -> TestResult {
    let dir = scratch("git")?;
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo)?;
    let mut interpreter = Interpreter::default();
    let git = |args: &[&str]| {
        Effect::Git(GitOp {
            cwd: text(&repo),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            stdin: None,
            timeout_ms: Some(60_000),
        })
    };
    let results = interpreter.perform_all(
        &[
            git(&["init", "-q"]),
            git(&["rev-parse", "--is-inside-work-tree"]),
            git(&["rev-parse", "--verify", "no-such-ref"]),
            git(&["hash-object", "--stdin"]),
        ]
        .into_iter()
        .collect(),
    );
    let outputs: Vec<_> = results
        .iter()
        .map(|result| match &result.outcome {
            EffectOutcome::Git(Ok(output)) => Ok(output.clone()),
            other => Err(format!("unexpected outcome {other:?}")),
        })
        .collect::<Result<_, _>>()?;
    assert_eq!(outputs[0].exit_code, 0);
    assert_eq!(
        (outputs[1].exit_code, outputs[1].stdout.trim()),
        (0, "true")
    );
    assert_ne!(outputs[2].exit_code, 0);
    // No stdin: git hashes the empty blob.
    assert_eq!(
        outputs[3].stdout.trim(),
        "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
    );

    let with_stdin = interpreter.perform(&Effect::Git(GitOp {
        stdin: Some("hello\n".to_owned()),
        ..match git(&["hash-object", "--stdin"]) {
            Effect::Git(op) => op,
            _ => return Err("not git".into()),
        }
    }));
    match with_stdin.outcome {
        EffectOutcome::Git(Ok(output)) => assert_eq!(
            output.stdout.trim(),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        ),
        other => return Err(format!("unexpected outcome {other:?}").into()),
    }

    let log = dir.join("logs").join("spawn.log");
    let spawned = interpreter.perform(&Effect::Spawn(ProcessSpec {
        argv: VectorSync::new_sync()
            .push_back("git".to_owned())
            .push_back("--version".to_owned()),
        cwd: Some(text(&dir)),
        env: VectorSync::new_sync(),
        stdin: None,
        timeout_ms: Some(60_000),
        log_path: Some(text(&log)),
    }));
    match spawned.outcome {
        EffectOutcome::Spawn(Ok(exit)) => {
            assert_eq!(exit.exit_code, Some(0));
            assert!(!exit.timed_out);
            assert!(exit.stdout.starts_with("git version"), "{}", exit.stdout);
        }
        other => return Err(format!("unexpected outcome {other:?}").into()),
    }
    assert!(std::fs::read_to_string(&log)?.starts_with("git version"));

    let missing = interpreter.perform(&Effect::Spawn(ProcessSpec {
        argv: VectorSync::new_sync().push_back("wg-no-such-program-sh23".to_owned()),
        cwd: None,
        env: VectorSync::new_sync(),
        stdin: None,
        timeout_ms: None,
        log_path: None,
    }));
    assert!(matches!(
        missing.outcome,
        EffectOutcome::Spawn(Err(wg_model::EffectError {
            kind: wg_model::EffectErrorKind::NotFound,
            ..
        }))
    ));
    Ok(())
}

#[test]
fn fs_effects_and_missing_http_client() -> TestResult {
    let dir = scratch("fs")?;
    let mut interpreter = Interpreter::default();
    let spool = dir.join("spool").join("hooks.jsonl");
    let sub = dir.join("a").join("b");
    let effects: VectorSync<Effect> = [
        Effect::Fs(FsOp::AppendLine {
            path: text(&spool),
            line: "{\"n\":1}".to_owned(),
        }),
        Effect::Fs(FsOp::AppendLine {
            path: text(&spool),
            line: "{\"n\":2}".to_owned(),
        }),
        Effect::Fs(FsOp::CreateDir { path: text(&sub) }),
        Effect::Fs(FsOp::RemoveFile {
            path: text(&dir.join("absent")),
        }),
        Effect::Fs(FsOp::WriteAtomic {
            path: text(&sub.join("f.txt")),
            contents: "one".to_owned(),
        }),
        Effect::Fs(FsOp::WriteAtomic {
            path: text(&sub.join("f.txt")),
            contents: "two".to_owned(),
        }),
        Effect::Http(wg_model::HttpCall {
            method: wg_model::HttpMethod::Get,
            url: "http://127.0.0.1:9/".to_owned(),
            headers: VectorSync::new_sync(),
            body: None,
            timeout_ms: None,
        }),
    ]
    .into_iter()
    .collect();
    let results = interpreter.perform_all(&effects);
    assert!(
        results[..6]
            .iter()
            .all(|r| r.outcome == EffectOutcome::Fs(Ok(())))
    );
    assert_eq!(std::fs::read_to_string(&spool)?, "{\"n\":1}\n{\"n\":2}\n");
    assert_eq!(std::fs::read_to_string(sub.join("f.txt"))?, "two");
    // No temporary files are left behind.
    assert_eq!(std::fs::read_dir(&sub)?.count(), 1);
    assert!(matches!(
        &results[6].outcome,
        EffectOutcome::Http(Err(err)) if err.kind == wg_model::EffectErrorKind::Unavailable
    ));

    let removed = interpreter.perform(&Effect::Fs(FsOp::RemoveDir {
        path: text(&dir.join("a")),
    }));
    assert_eq!(removed.outcome, EffectOutcome::Fs(Ok(())));
    assert!(!dir.join("a").exists());
    Ok(())
}

#[test]
fn bus_wakes_waiters_and_publishes_per_workspace() -> TestResult {
    let mut interpreter = Interpreter::default();
    let bus = interpreter.bus.clone();
    let seen = bus.wakes();
    let waiter = std::thread::spawn(move || bus.wait_for_wake(seen, Duration::from_secs(30)));
    std::thread::sleep(Duration::from_millis(50));
    interpreter.perform(&Effect::Wake(JobWake {
        workspace_id: None,
        kind: "verify.run".to_owned(),
    }));
    assert_eq!(waiter.join().map_err(|_| "waiter panicked")?, seen + 1);
    // Nothing new: the wait times out with the same counter.
    assert_eq!(
        interpreter
            .bus
            .wait_for_wake(seen + 1, Duration::from_millis(20)),
        seen + 1
    );

    let ours = wg_model::derive_id(&[1; 32], 0);
    let theirs = wg_model::derive_id(&[1; 32], 1);
    let stream = interpreter.bus.subscribe(ours);
    let dropped = interpreter.bus.subscribe(ours);
    drop(dropped);
    for workspace_id in [theirs, ours] {
        interpreter.perform(&Effect::Publish(StreamEvent {
            workspace_id,
            name: "domain".to_owned(),
            data: serde_json::json!({"type": "task.created"}),
        }));
    }
    let received: Vec<StreamEvent> = stream.try_iter().collect();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].workspace_id, ours);
    Ok(())
}

/// A toy flow: start, ask git whether the directory is a work tree, record the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Start,
    Probing,
    Done(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProbeState {
    dir: String,
    phase: Phase,
}

enum ProbeInput {
    Begin,
    Answer(Box<EffectResult>),
}

fn answer(result: EffectResult) -> ProbeInput {
    ProbeInput::Answer(Box::new(result))
}

fn probe_step(
    state: ProbeState,
    input: &ProbeInput,
    events_so_far: &i64,
    stamp: &Stamp,
) -> Result<(ProbeState, Decision<()>), WgError> {
    match (&state.phase, input) {
        (Phase::Start, ProbeInput::Begin) => {
            let probe = Effect::Git(GitOp {
                cwd: state.dir.clone(),
                args: ["rev-parse", "--is-inside-work-tree"]
                    .iter()
                    .map(|a| (*a).to_owned())
                    .collect(),
                stdin: None,
                timeout_ms: Some(60_000),
            });
            Ok((
                ProbeState {
                    phase: Phase::Probing,
                    ..state
                },
                Decision::new(()).effect(probe),
            ))
        }
        (Phase::Probing, ProbeInput::Answer(result)) => {
            let inside = matches!(
                &result.outcome,
                EffectOutcome::Git(Ok(output)) if output.stdout.trim() == "true"
            );
            let slug = format!("probe-{events_so_far}-{inside}");
            let decision = create_workspace(&slug, stamp, &[])?.with_reply(());
            Ok((
                ProbeState {
                    phase: Phase::Done(inside),
                    ..state
                },
                decision,
            ))
        }
        _ => Err(WgError::new(
            ErrorCode::InvalidTransition,
            "unexpected input",
        )),
    }
}

#[test]
fn flow_driver_feeds_effect_results_back() -> TestResult {
    let dir = scratch("flow")?;
    let mut runtime = runtime(&dir)?;
    let flow = Flow {
        name: "probe",
        load: |session, _state: &ProbeState| count(session, "events"),
        step: probe_step,
        input: answer,
        finished: |state: &ProbeState| matches!(state.phase, Phase::Done(_)),
    };
    let start = ProbeState {
        dir: text(&dir),
        phase: Phase::Start,
    };
    let done = runtime.drive(&flow, &context()?, start, ProbeInput::Begin)?;
    // The scratch directory sits inside the repository's own work tree.
    assert_eq!(done.phase, Phase::Done(true));
    let events = runtime
        .store
        .read(|s| s.query_events(&EventQuery::default()))?;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].data.get("slug"),
        Some(&Value::String("probe-0-true".to_owned()))
    );

    // A finished flow takes no further step.
    let again = runtime.drive(&flow, &context()?, done.clone(), ProbeInput::Begin)?;
    assert_eq!(again, done);
    Ok(())
}

// covers: O14
#[test]
fn logging_writes_o14_lines_to_the_log_file() -> TestResult {
    let dir = scratch("logging")?;
    let layer = json_layer(&LogSettings {
        level: Level::Info,
        log_dir: Some(dir.clone()),
        stderr: false,
    })?;
    let subscriber = tracing_subscriber_registry(layer);
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "workgraph.jobs.worker", job_id = "j1", attempt = 3_i64, ok = true, "claimed job");
        tracing::debug!(target: "workgraph.jobs.worker", "below the level");
        tracing::warn!(target: "rusqlite", "outside the tree");
        tracing::warn!(target: "workgraph", ratio = 0.5_f64, "half");
    });
    let content = std::fs::read_to_string(dir.join("workgraph.log"))?;
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2, "{content}");
    let first: serde_json::Map<String, Value> = serde_json::from_str(lines[0])?;
    let keys: Vec<&str> = first.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["ts", "level", "logger", "msg", "job_id", "attempt", "ok"]
    );
    assert_eq!(first["level"], "info");
    assert_eq!(first["logger"], "workgraph.jobs.worker");
    assert_eq!(first["msg"], "claimed job");
    assert_eq!(first["attempt"], 3);
    assert!(
        first["ts"]
            .as_str()
            .is_some_and(|ts| ts.ends_with("+00:00"))
    );
    assert!(lines[0].contains("\"job_id\": \"j1\", \"attempt\": 3"));
    assert!(lines[1].contains("\"level\": \"warning\""));
    assert!(lines[1].ends_with("\"ratio\": 0.5}"));
    if cfg!(windows) {
        assert!(content.contains("}\r\n"));
    }
    Ok(())
}

fn tracing_subscriber_registry(
    layer: wg_runtime::JsonLayer,
) -> impl tracing::Subscriber + Send + Sync {
    use tracing_subscriber::layer::SubscriberExt as _;
    tracing_subscriber::registry::Registry::default().with(layer)
}

#[test]
fn log_file_rotates_like_rotating_file_handler() -> TestResult {
    let dir = scratch("rotation")?;
    let path = dir.join("workgraph.log");
    let mut file = RotatingFile::open(&path, 100, 2)?;
    let line = "x".repeat(39);
    for _ in 0..7 {
        file.write_line(&line)?;
    }
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let row = 40 + u64::from(cfg!(windows));
    // Two lines fit under 100 bytes; the third would reach it and rolls the file over.
    assert_eq!(size(&path), row);
    assert_eq!(size(&dir.join("workgraph.log.1")), 2 * row);
    assert_eq!(size(&dir.join("workgraph.log.2")), 2 * row);
    assert!(!dir.join("workgraph.log.3").exists());

    // Reopening continues at the current size.
    drop(file);
    let mut reopened = RotatingFile::open(&path, 100, 2)?;
    reopened.write_line(&line)?;
    assert_eq!(size(&path), 2 * row);
    Ok(())
}

#[test]
fn app_context_paths_token_config_and_workspaces() -> TestResult {
    let home = scratch("app")?;
    let mut app = App::open(Some(&home))?;
    for dir in [
        "config/workspaces",
        "data/artifacts",
        "data/spool/claude-hooks",
        "logs",
    ] {
        assert!(home.join(dir).is_dir(), "{dir}");
    }
    assert!(home.join("data").join("workgraph.db").is_file());

    let missing = app.api_token(false).err().ok_or("token existed")?;
    assert_eq!(missing.code, ErrorCode::ConfigError);
    let token = app.api_token(true)?;
    assert_eq!(token.len(), 43);
    assert_eq!(app.api_token(false)?, token);
    assert_eq!(
        std::fs::read_to_string(home.join("config").join("api-token"))?,
        token
    );

    let config = app.config()?;
    assert_eq!(config.get("schema_version"), Some(&Value::from(2)));
    std::fs::write(home.join("config").join("config.toml"), "not = [valid")?;
    let broken = app.config().err().ok_or("bad config loaded")?;
    assert_eq!(broken.code, ErrorCode::ConfigError);
    assert!(broken.message.contains("is not valid TOML"));
    std::fs::remove_file(home.join("config").join("config.toml"))?;

    create_schema(&mut app.runtime.store)?;
    let none = app
        .default_workspace_id()
        .err()
        .ok_or("found a workspace")?;
    assert_eq!(none.code, ErrorCode::NotFound);
    let ctx = context()?;
    let ops = app
        .runtime
        .decide(
            &ctx,
            |_| Ok(()),
            |(), stamp| create_workspace("ops", stamp, &[]),
        )?
        .reply;
    assert_eq!(app.default_workspace_id()?, ops);
    assert_eq!(app.workspace_id("ops")?, ops);
    assert_eq!(app.workspace_id(&ops.to_string())?, ops);
    assert_eq!(app.workspace_slug(&ops.to_string())?, "ops");
    let unknown = app.workspace_id("nope").err().ok_or("resolved nope")?;
    assert_eq!(unknown.message, "workspace 'nope' not found");

    app.runtime.decide(
        &ctx,
        |_| Ok(()),
        |(), stamp| create_workspace("dev", stamp, &[]),
    )?;
    assert!(app.default_workspace_id().is_err());
    std::fs::write(
        home.join("config").join("config.toml"),
        "schema_version = 2\ndefault_workspace = \"ops\"\n",
    )?;
    assert_eq!(app.default_workspace_id()?, ops);
    Ok(())
}
