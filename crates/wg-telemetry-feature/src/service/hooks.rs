//! Hook ingestion and the hook spool (`integrations/claude_code/ingest.py` and `hook_cli.py`): the
//! shell half of P01, F09 and O15.
//!
//! A hook record is validated (`wg-telemetryspec`), normalised (`wg-telemetry`) and applied in one
//! `BEGIN IMMEDIATE` transaction: the dedupe check, the session and its binding, the hook ledger
//! row, then the effect of the event (tool events, session start and end, subagent runs and their
//! transcripts, internal tasks, the roll-up). The rows and events come from `wg_telemetry::ingest`;
//! this module reads what each step needs, asks for the row, and writes it.
//!
//! When ingestion fails the record is spooled to `<data>/spool/claude-hooks` and replayed later
//! (`hook_replay_spool`); a record that fails five replays is set aside as `.failed`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use wg_model::entity::{AgentAttempt, AgentSession, HookEvent, SessionBinding, SubagentRun, Task};
use wg_model::{EventDraft, EventPayload, IdSeq, Row, RowKey, Stamp, Write};
use wg_runtimespec::RequestContext;
use wg_store::{Location, Session, Store};
use wg_telemetry::binding::{
    BindingFacts, Resolved, attempt_row, binding_target, cwd_binding, mapped_task_key,
    resolve_binding, task_target, transcript_task, transcript_task_key,
};
use wg_telemetry::hook::{
    CaptureConfig, Normalised, capture_config_of, normalise, payload_object, task_keys_in,
    tool_event,
};
use wg_telemetry::ingest::{
    self, AttemptSpec, Finish, HookEnv, PROVIDER, SUBAGENT_ADAPTER, SUBAGENT_STOPPED,
    TRANSCRIPT_USAGE_SOURCE, TranscriptApplied, UNBOUND, canonical_id, hook_env, present,
};
pub use wg_telemetry::ingest::{Failure, IngestStatus};
use wg_telemetry::spool::{self, Replayed};
use wg_telemetry::transcript::{TranscriptStats, parse_transcript, sidecar_path, split_lines};
use wg_types::{AttemptStatus, Id, Permission, TaskKey, Timestamp, WgError};

use crate::service::sessions;
use crate::spec::failure::{failure_error, restore_failure};
use wg_runtime::{
    App, Interpreter, Runtime, SystemStamps, resolve_paths, system_context, write_spooled,
};

/// `IngestResult`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IngestResult {
    pub status: IngestStatus,
    pub hook_event_id: Option<Id>,
    pub session_id: Option<Id>,
    pub attempt_id: Option<Id>,
}

/// What `replay_spool` did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayCounts {
    pub ingested: u64,
    pub duplicate: u64,
    pub failed: u64,
}

/// `parse_transcript(path)`: the transcript's lines (lossy UTF-8, `None` when unreadable) and its
/// sidecar (strict UTF-8, ignored when unreadable).
fn read_stats(path: &str, offset: i32) -> Option<TranscriptStats> {
    let lines = std::fs::read(path)
        .ok()
        .map(|bytes| split_lines(&String::from_utf8_lossy(&bytes)));
    let sidecar = std::fs::read(sidecar_path(path))
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());
    parse_transcript(lines.as_deref(), sidecar.as_deref(), offset)
}

/// The work of one ingestion transaction.
struct Ingestion<'a, 'c> {
    session: &'a mut Session<'c>,
    stamp: &'a Stamp,
    ids: IdSeq,
    offset: i32,
}

/// The local zone's offset from UTC in seconds, for the naive timestamps of a transcript (0 when the
/// platform cannot tell).
fn local_offset() -> i32 {
    let now = time::OffsetDateTime::from(std::time::SystemTime::now());
    time::UtcOffset::local_offset_at(now).map_or(0, |offset| offset.whole_seconds())
}

impl<'a, 'c> Ingestion<'a, 'c> {
    /// The work of a transaction stamped with `stamp`.
    fn start(session: &'a mut Session<'c>, stamp: &'a Stamp) -> Self {
        let offset = local_offset();
        Ingestion {
            session,
            stamp,
            ids: stamp.ids(),
            offset,
        }
    }

    /// The next id of the transaction.
    fn id(&mut self) -> Id {
        let (id, next) = self.ids.next();
        self.ids = next;
        id
    }

    fn put(&mut self, row: impl Into<Row>) -> Result<(), WgError> {
        self.session.apply_write(&Write::insert(row))
    }

    fn update(&mut self, row: impl Into<Row>) -> Result<(), WgError> {
        self.session.apply_write(&Write::update(row))
    }

    /// `record_event`.
    fn event(
        &mut self,
        workspace: Option<Id>,
        entity: Id,
        payload: EventPayload,
        attempt: Option<Id>,
    ) -> Result<(), WgError> {
        let id = self.id();
        let draft =
            EventDraft::new(self.stamp, id, workspace, entity, payload).with_attempt(attempt);
        self.session.append_event(&draft).map(|_| ())
    }

    fn append(&mut self, draft: &EventDraft) -> Result<(), WgError> {
        self.session.append_event(draft).map(|_| ())
    }

    /// `process_record` after the validation: the dedupe check, the session, the ledger row and the
    /// event's effect.
    fn ingest(
        &mut self,
        normalised: &Normalised,
        validated: &Map<String, Value>,
        env: &HookEnv,
        source: &str,
    ) -> Result<IngestResult, Failure> {
        if sessions::hook_event_id_by_dedupe_key(self.session, &normalised.dedupe_key)?.is_some() {
            return Ok(IngestResult {
                status: IngestStatus::Duplicate,
                hook_event_id: None,
                session_id: None,
                attempt_id: None,
            });
        }
        let at = normalised.received_at.unwrap_or(self.stamp.at);
        let agent = self.agent_session(normalised, env, at)?;
        let hook_id = self.id();
        let hook = ingest::hook_row(hook_id, normalised, agent.as_ref(), at, source);
        self.put(hook.clone())?;
        self.apply(normalised, validated, &hook, agent.as_ref(), at)?;
        Ok(IngestResult {
            status: IngestStatus::Ingested,
            hook_event_id: Some(hook.id),
            session_id: agent.map(|session| session.id),
            attempt_id: hook.attempt_id,
        })
    }

    /// The session of the payload: found, created and bound, or re-bound when it was unbound.
    fn agent_session(
        &mut self,
        normalised: &Normalised,
        env: &HookEnv,
        at: Timestamp,
    ) -> Result<Option<AgentSession>, WgError> {
        let fields = &normalised.fields;
        let Some(external) = present(fields.session_id.as_ref()) else {
            return Ok(None);
        };
        let now = self.stamp.at;
        match sessions::agent_session_by_external(self.session, PROVIDER, external)? {
            None => {
                let resolved = self.resolve(env, external, fields.cwd.as_deref())?;
                let id = self.id();
                let row = ingest::new_session(id, external, &resolved, fields, at, now);
                self.put(row.clone())?;
                Ok(Some(row))
            }
            Some(row) if row.binding_source == UNBOUND => {
                let resolved = self.resolve(env, external, fields.cwd.as_deref())?;
                match ingest::rebound(&row, &resolved, now) {
                    Some(bound) => {
                        self.update(bound.clone())?;
                        Ok(Some(bound))
                    }
                    None => Ok(Some(row)),
                }
            }
            Some(row) => Ok(Some(row)),
        }
    }

    /// `_resolve_binding`: reads the rows the binding rules look at, then applies them.
    fn resolve(
        &mut self,
        env: &HookEnv,
        external: &str,
        cwd: Option<&str>,
    ) -> Result<Resolved, WgError> {
        let env_attempt = match env.attempt_id.as_deref().and_then(canonical_id) {
            Some(id) => self.session.attempt_by_id(id)?.as_ref().map(attempt_row),
            None => None,
        };
        let env_task = match env.task_id.as_deref().and_then(canonical_id) {
            Some(id) => self
                .session
                .fetch::<Task>(&RowKey::Task(id))?
                .as_ref()
                .map(task_target),
            None => None,
        };
        let facts = BindingFacts {
            env_attempt,
            session_binding: sessions::active_binding_by_external(self.session, external)?
                .as_ref()
                .map(binding_target),
            cwd_bindings: sessions::active_cwd_bindings(self.session)?
                .iter()
                .map(cwd_binding)
                .collect(),
            env_task,
        };
        Ok(resolve_binding(&facts, Some(external), cwd))
    }

    /// `_apply`.
    fn apply(
        &mut self,
        normalised: &Normalised,
        validated: &Map<String, Value>,
        hook: &HookEvent,
        agent: Option<&AgentSession>,
        at: Timestamp,
    ) -> Result<(), Failure> {
        let fields = &normalised.fields;
        let now = self.stamp.at;
        if let Some(tool) = tool_event(fields, &normalised.tool_summary) {
            if ingest::duration_overflows(validated, fields) {
                let (type_name, message) = ingest::overflow_failure();
                return Err(Failure::new(type_name, message));
            }
            let phase = tool.phase;
            let id = self.id();
            self.put(ingest::tool_row(id, hook.id, agent, fields, tool, at))?;
            if let Some(session) = agent.filter(|_| phase != "pre") {
                self.update(ingest::counted(session, phase, now))?;
            }
            return Ok(());
        }
        let Some(agent) = agent else {
            return Ok(());
        };
        match fields.hook_event_name.as_str() {
            "SessionStart" => {
                let started = ingest::started(agent, fields, at, now);
                self.update(started.clone())?;
                let payload = ingest::started_event(&started, fields);
                self.event(agent.workspace_id, agent.id, payload, agent.attempt_id)?;
            }
            "SessionEnd" => {
                let ended = ingest::ended(agent, fields, at, now);
                self.update(ended.clone())?;
                self.roll_up(&ended)?;
                let payload = ingest::ended_event(&ended);
                self.event(agent.workspace_id, agent.id, payload, agent.attempt_id)?;
            }
            "SubagentStart" | "SubagentStop" => {
                if let Some(agent_id) = present(fields.agent_id.as_ref()) {
                    self.subagent(normalised, agent, agent_id, at)?;
                }
            }
            "TaskCreated" | "TaskCompleted" => {
                if let Some(task_id) = present(fields.task_id.as_ref()) {
                    self.internal_task(normalised, agent, task_id, at)?;
                }
            }
            "Stop" => self.roll_up(agent)?,
            _ => {}
        }
        Ok(())
    }

    /// `_roll_up`: the session's hook counters on its attempt's telemetry.
    fn roll_up(&mut self, agent: &AgentSession) -> Result<(), WgError> {
        let Some(attempt_id) = agent.attempt_id else {
            return Ok(());
        };
        let Some(attempt) = self.session.attempt_by_id(attempt_id)? else {
            return Ok(());
        };
        let runs = sessions::subagent_run_count_for_session(self.session, agent.id)?;
        let count = usize::try_from(runs).unwrap_or_default();
        self.update(ingest::rolled_up(&attempt, agent, count, self.stamp.at))
    }

    /// A `SubagentStart` or `SubagentStop`.
    fn subagent(
        &mut self,
        normalised: &Normalised,
        agent: &AgentSession,
        agent_id: &str,
        at: Timestamp,
    ) -> Result<(), WgError> {
        let fields = &normalised.fields;
        let found = sessions::subagent_run_by_external(self.session, agent.id, agent_id)?;
        let run = match found {
            Some(run) => run,
            None => {
                let id = self.id();
                let run = ingest::new_run(id, agent, agent_id, fields, at);
                self.put(run.clone())?;
                match agent.attempt_id {
                    Some(parent) => match self.child_attempt(parent, agent_id, fields, at)? {
                        Some(child) => {
                            let linked = SubagentRun {
                                attempt_id: Some(child),
                                ..run
                            };
                            self.update(linked.clone())?;
                            linked
                        }
                        None => run,
                    },
                    None => run,
                }
            }
        };
        let run = if fields.hook_event_name == "SubagentStop" {
            let stopped = ingest::stopped(&run, fields, at);
            self.update(stopped.clone())?;
            let (run, applied) = self.apply_transcript(
                stopped,
                Some(&agent.external_session_id),
                agent.workspace_id,
                at,
            )?;
            if !applied.attempt_bound {
                self.finish_child(&run)?;
            }
            run
        } else {
            run
        };
        let payload = ingest::subagent_event(&run, fields);
        self.event(agent.workspace_id, agent.id, payload, agent.attempt_id)
    }

    /// A subagent stop finishes the child attempt its start created, unless the transcript bound the
    /// run to an attempt the orchestrator owns.
    fn finish_child(&mut self, run: &SubagentRun) -> Result<(), WgError> {
        let Some(child_id) = run.attempt_id else {
            return Ok(());
        };
        match self.session.attempt_by_id(child_id)? {
            Some(child) if ingest::finishes_with_subagent(&child) => {
                let finish = Finish {
                    exit_reason: Some(SUBAGENT_STOPPED.to_owned()),
                    summary: run.last_message_excerpt.clone(),
                    ..Finish::default()
                };
                self.finish(&child, finish)
            }
            _ => Ok(()),
        }
    }

    /// `finish_attempt(attempt, succeeded, ...)`.
    fn finish(&mut self, attempt: &AgentAttempt, finish: Finish) -> Result<(), WgError> {
        let event_id = self.id();
        match ingest::finish_attempt(
            self.stamp,
            event_id,
            attempt,
            AttemptStatus::Succeeded,
            finish,
        )? {
            Some((finished, draft)) => {
                self.update(finished)?;
                self.append(&draft)
            }
            None => Ok(()),
        }
    }

    /// The number of the next attempt of a task.
    fn next_number(&mut self, task_id: Id) -> Result<i64, WgError> {
        Ok(ingest::next_number(
            &self.session.attempts_of_task(task_id, &[])?,
        ))
    }

    /// `new_attempt(...)`: writes the attempt and its `attempt.created` event.
    fn create_attempt(&mut self, task: &Task, spec: AttemptSpec) -> Result<AgentAttempt, WgError> {
        let number = self.next_number(task.id)?;
        let ids = (self.id(), self.id());
        let (attempt, created) = ingest::new_attempt(self.stamp, ids, task, number, spec);
        self.put(attempt.clone())?;
        self.append(&created)?;
        Ok(attempt)
    }

    /// `_child_attempt`: the attempt of a subagent the orchestrator's attempt started.
    fn child_attempt(
        &mut self,
        parent_id: Id,
        agent_id: &str,
        fields: &wg_telemetry::HookFields,
        at: Timestamp,
    ) -> Result<Option<Id>, WgError> {
        let Some(parent) = self.session.attempt_by_id(parent_id)? else {
            return Ok(None);
        };
        let Some(task) = self.session.fetch::<Task>(&RowKey::Task(parent.task_id))? else {
            return Ok(None);
        };
        let spec = AttemptSpec {
            adapter: SUBAGENT_ADAPTER,
            agent_id: ingest::subagent_agent_id(fields.agent_type.as_deref()),
            model: None,
            session_id: fields.session_id.clone(),
            parent_attempt_id: Some(parent.id),
            started_at: Some(at),
            telemetry: ingest::child_telemetry(agent_id, fields.agent_type.as_deref()),
        };
        Ok(Some(self.create_attempt(&task, spec)?.id))
    }

    /// `_transcript_task`: the task named by the first `Task-ID:` line, when exactly one in scope has
    /// that key.
    fn transcript_task(
        &mut self,
        workspace: Option<Id>,
        stats: &TranscriptStats,
    ) -> Result<Option<Task>, WgError> {
        let Some(key) =
            transcript_task_key(&stats.task_keys).and_then(|key| TaskKey::parse(key).ok())
        else {
            return Ok(None);
        };
        let matches = match workspace {
            Some(scope) => self.session.task_by_key(scope, &key)?.into_iter().collect(),
            None => self.session.tasks_by_key(&key)?,
        };
        Ok(transcript_task(&stats.task_keys, matches))
    }

    /// `_apply_transcript`: fills a subagent run from its transcript. Shared by the stop hook and the
    /// backfill.
    fn apply_transcript(
        &mut self,
        run: SubagentRun,
        external_session_id: Option<&str>,
        workspace: Option<Id>,
        at: Timestamp,
    ) -> Result<(SubagentRun, TranscriptApplied), WgError> {
        let none = TranscriptApplied::default();
        let Some(path) = run.transcript_path.clone().filter(|path| !path.is_empty()) else {
            return Ok((run, none));
        };
        let Some(stats) = read_stats(&path, self.offset) else {
            return Ok((run, none));
        };
        let now = self.stamp.at;
        let task = self.transcript_task(workspace, &stats)?;
        let run = ingest::run_with_transcript(&run, &stats, task.as_ref());
        let applied = TranscriptApplied {
            parsed: true,
            linked: task.is_some(),
            ..TranscriptApplied::default()
        };
        if let Some(child_id) = run.attempt_id {
            if let Some(child) = self.session.attempt_by_id(child_id)? {
                self.update(ingest::child_with_transcript(&child, &stats, now))?;
            }
            self.update(run.clone())?;
            return Ok((run, applied));
        }
        let Some(task) = task else {
            self.update(run.clone())?;
            return Ok((run, applied));
        };
        if let Some(open) = sessions::open_attempt_for_subagent(self.session, task.id)? {
            let run = SubagentRun {
                attempt_id: Some(open.id),
                ..run
            };
            self.update(run.clone())?;
            let bound = ingest::bound_attempt(&open, &stats, now);
            self.update(bound.clone())?;
            let id = self.id();
            let draft = ingest::subagent_bound_event(self.stamp, id, &bound, &run, &task, &stats);
            self.append(&draft)?;
            return Ok((
                run,
                TranscriptApplied {
                    attempt_bound: true,
                    ..applied
                },
            ));
        }
        let spec = AttemptSpec {
            adapter: SUBAGENT_ADAPTER,
            agent_id: ingest::subagent_agent_id(ingest::transcript_agent_type(&stats, &run)),
            model: stats.model.clone(),
            session_id: external_session_id.map(str::to_owned),
            parent_attempt_id: None,
            started_at: Some(ingest::transcript_start(&stats, &run, at)),
            telemetry: Map::new(),
        };
        let attempt = self.create_attempt(&task, spec)?;
        let run = SubagentRun {
            attempt_id: Some(attempt.id),
            ..run
        };
        self.update(run.clone())?;
        let usage = ingest::transcript_usage(&stats);
        let finish = Finish {
            exit_reason: Some(SUBAGENT_STOPPED.to_owned()),
            summary: run.last_message_excerpt.clone(),
            usage_source: usage.as_ref().map(|_| TRANSCRIPT_USAGE_SOURCE.to_owned()),
            usage,
            telemetry: Some(ingest::transcript_telemetry(&stats)),
        };
        self.finish(&attempt, finish)?;
        Ok((
            run,
            TranscriptApplied {
                attempt_created: true,
                ..applied
            },
        ))
    }

    /// A `TaskCreated` or `TaskCompleted`.
    fn internal_task(
        &mut self,
        normalised: &Normalised,
        agent: &AgentSession,
        task_id: &str,
        at: Timestamp,
    ) -> Result<(), WgError> {
        let fields = &normalised.fields;
        let found = sessions::internal_task_by_external(self.session, agent.id, task_id)?;
        let existed = found.is_some();
        let task = match found {
            Some(task) => task,
            None => {
                let id = self.id();
                ingest::new_internal_task(id, agent, task_id, fields, self.stamp.at)
            }
        };
        let mapped = self.map_internal_task(agent.workspace_id, fields)?;
        let updated = ingest::internal_task_updated(&task, mapped, fields, at);
        if existed {
            self.update(updated.clone())?;
        } else {
            self.put(updated.clone())?;
        }
        let payload = ingest::internal_task_event(&updated, fields);
        self.event(agent.workspace_id, agent.id, payload, agent.attempt_id)
    }

    /// `_map_internal_task`: links an internal task to a Workgraph task only when it names one.
    fn map_internal_task(
        &mut self,
        workspace: Option<Id>,
        fields: &wg_telemetry::HookFields,
    ) -> Result<Option<Id>, WgError> {
        let Some(scope) = workspace else {
            return Ok(None);
        };
        let text = format!(
            "{} {}",
            fields.task_subject.as_deref().unwrap_or(""),
            fields.task_description.as_deref().unwrap_or("")
        );
        let mut known = std::collections::BTreeMap::new();
        for name in task_keys_in(&text) {
            if let Ok(key) = TaskKey::parse(&name)
                && let Some(task) = self.session.task_by_key(scope, &key)?
            {
                known.insert(name, task.id);
            }
        }
        let chosen = mapped_task_key(
            fields.task_subject.as_deref(),
            fields.task_description.as_deref(),
            |key| known.contains_key(key),
        );
        Ok(chosen.and_then(|key| known.get(&key).copied()))
    }
}

/// `HookIngestor.ingest`: one record, stamped as the `claude-hooks` system component.
pub fn hook_ingest(app: &mut App, record: &Value, source: &str) -> Result<IngestResult, Failure> {
    let config = capture_config_of(&app.config()?);
    hook_ingest_with(app, config, record, source)
}

/// `process_record`: validates and normalises the record, then applies it in one transaction.
pub fn hook_ingest_with(
    app: &mut App,
    config: CaptureConfig,
    record: &Value,
    source: &str,
) -> Result<IngestResult, Failure> {
    let raw = payload_object(record);
    let validated = match wg_telemetryspec::validate_hook_payload(&raw) {
        Ok(Value::Object(validated)) => validated,
        Ok(_) => Map::new(),
        Err(errors) => {
            return Err(Failure::new(
                "ValidationError",
                wg_telemetryspec::failure_message(&errors),
            ));
        }
    };
    let normalised = normalise(record, &validated, config, source)
        .map_err(|error| Failure::new("ValueError", error.message))?;
    let env = hook_env(record);
    let context = system_context("claude-hooks")?;
    let stamps = &mut app.runtime.stamps;
    app.runtime
        .store
        .write(|session| {
            let stamp = stamps.stamp(&context)?;
            let mut work = Ingestion::start(session, &stamp);
            work.ingest(&normalised, &validated, &env, source)
                .map_err(|failure| failure_error(&failure))
        })
        .map_err(restore_failure)
}

/// `HookIngestor.replay_spool`: ingests spooled records oldest first (`limit` of them).
pub fn hook_replay_spool(app: &mut App, limit: usize) -> Result<ReplayCounts, WgError> {
    let dir = PathBuf::from(app.paths.spool_dir().as_str());
    let config = capture_config_of(&app.config()?);
    let mut files = spool_files(&dir, "json");
    files.sort();
    let mut counts = ReplayCounts::default();
    for file in files.into_iter().take(limit) {
        let record = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(Value::is_object);
        let Some(record) = record else {
            counts.failed += 1;
            let _ = std::fs::rename(&file, file.with_extension("failed"));
            continue;
        };
        match hook_ingest_with(app, config, &record, "spool") {
            Ok(result) => {
                match result.status {
                    IngestStatus::Duplicate => counts.duplicate += 1,
                    IngestStatus::Ingested => counts.ingested += 1,
                }
                let _ = std::fs::remove_file(&file);
            }
            Err(failure) => {
                counts.failed += 1;
                match spool::after_failed_replay(&record, &failure.reason()) {
                    Replayed::Retry(updated) => write_spooled(&file, &updated)?,
                    Replayed::SetAside(updated) => {
                        write_spooled(&file.with_extension("failed"), &updated)?;
                        let _ = std::fs::remove_file(&file);
                    }
                }
            }
        }
    }
    Ok(counts)
}

/// `HookIngestor.stats`.
pub fn hook_stats(app: &mut App) -> Result<Value, WgError> {
    let dir = PathBuf::from(app.paths.spool_dir().as_str());
    let (total, last, by_event) = app.runtime.store.read(|session| {
        Ok((
            sessions::hook_event_count(session)?,
            sessions::hook_event_last_received(session)?,
            sessions::hook_event_counts_by_name(session)?,
        ))
    })?;
    Ok(json!({
        "hook_events": total,
        "last_received_at": last.map(Timestamp::format_isoformat),
        "by_event": by_event
            .into_iter()
            .map(|row| (row.hook_event_name, json!(row.count)))
            .collect::<Map<String, Value>>(),
        "spooled": spool_files(&dir, "json").len(),
        "spool_failed": spool_files(&dir, "failed").len(),
    }))
}

/// `HookIngestor.sessions`: the newest sessions, newest first.
pub fn hook_sessions(
    app: &mut App,
    workspace: Option<Id>,
    limit: i64,
) -> Result<Vec<Value>, WgError> {
    let rows = app.runtime.store.read(|session| {
        sessions::agent_session_summaries(session, PROVIDER, workspace, limit, 0)
    })?;
    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id.to_string(),
                "external_session_id": row.external_session_id,
                "binding": row.binding_source,
                "task_id": row.task_id.map(|id| id.to_string()),
                "attempt_id": row.attempt_id.map(|id| id.to_string()),
                "cwd": row.cwd,
                "model": row.model,
                "tool_calls": row.tool_calls,
                "tool_failures": row.tool_failures,
                "started_at": row.started_at.map(Timestamp::format_isoformat),
                "ended_at": row.ended_at.map(Timestamp::format_isoformat),
                "end_reason": row.end_reason,
            })
        })
        .collect())
}

/// `HookIngestor.unbind`: deactivates the bindings named by id or working directory; returns how
/// many.
pub fn hook_unbind(
    app: &mut App,
    binding_id: Option<Id>,
    cwd: Option<&str>,
) -> Result<usize, WgError> {
    app.runtime.store.write(|session| {
        let rows = sessions::active_bindings(session, binding_id, cwd)?;
        let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
        sessions::deactivate_bindings(session, &ids)
    })
}

/// `HookIngestor.backfill_transcripts`: applies transcript telemetry to subagent runs that have no
/// model yet. Idempotent.
pub fn hook_backfill_transcripts(
    app: &mut App,
    context: &RequestContext,
    limit: Option<i64>,
) -> Result<Value, WgError> {
    context.require(&[Permission::TelemetryIngest])?;
    let stamps = &mut app.runtime.stamps;
    app.runtime.store.write(|session| {
        let stamp = stamps.stamp(context)?;
        let runs = sessions::subagent_runs_needing_model(session, limit)?;
        let mut counts = [0_i64; 4];
        let mut work = Ingestion::start(session, &stamp);
        for run in runs {
            let owner = work
                .session
                .fetch::<AgentSession>(&RowKey::AgentSession(run.session_id))?;
            let (_, applied) = work.apply_transcript(
                run,
                owner
                    .as_ref()
                    .map(|owner| owner.external_session_id.as_str()),
                owner.as_ref().and_then(|owner| owner.workspace_id),
                stamp.at,
            )?;
            counts[usize::from(!applied.parsed)] += 1;
            counts[2] += i64::from(applied.linked);
            counts[3] += i64::from(applied.attempt_created);
        }
        Ok(json!({
            "parsed": counts[0],
            "missing": counts[1],
            "linked": counts[2],
            "attempts_created": counts[3],
        }))
    })
}

/// `HookIngestor.backfill_time`: stores the wall-time breakdown on subagent runs whose telemetry
/// lacks it. Only `telemetry["time"]` is written. Idempotent.
pub fn hook_backfill_time(
    app: &mut App,
    context: &RequestContext,
    limit: Option<i64>,
) -> Result<Value, WgError> {
    context.require(&[Permission::TelemetryIngest])?;
    app.runtime.store.write(|session| {
        let offset = local_offset();
        let runs = sessions::subagent_runs_with_transcript(session, limit)?;
        let mut counts = [0_i64; 3];
        for run in runs {
            let telemetry = run.telemetry.clone().unwrap_or_default();
            if telemetry.contains_key("time") {
                counts[2] += 1;
                continue;
            }
            let path = run.transcript_path.clone().unwrap_or_default();
            match read_stats(&path, offset).filter(|stats| !stats.time.is_null()) {
                Some(stats) => {
                    session.apply_write(&Write::update(SubagentRun {
                        telemetry: Some(ingest::with_time(&telemetry, &stats)),
                        ..run
                    }))?;
                    counts[0] += 1;
                }
                None => counts[1] += 1,
            }
        }
        Ok(json!({"parsed": counts[0], "missing": counts[1], "skipped": counts[2]}))
    })
}

/// `HookIngestor.bind`: binds an external session id or a working directory to a task.
pub fn hook_bind(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_ref: &str,
    session_id: Option<&str>,
    cwd: Option<&str>,
) -> Result<Value, WgError> {
    context.require(&[Permission::TelemetryIngest])?;
    let scope = app.workspace_or_default(workspace)?;
    let cwd = cwd
        .map(str::to_owned)
        .or_else(|| {
            session_id
                .is_none()
                .then(|| std::env::current_dir().ok())
                .flatten()
                .map(|dir| dir.to_string_lossy().into_owned())
        })
        .filter(|path| !path.is_empty())
        .map(|path| resolved_path(&path));
    let session_id = session_id.filter(|id| !id.is_empty());
    let stamps = &mut app.runtime.stamps;
    app.runtime.store.write(|session| {
        let stamp = stamps.stamp(context)?;
        let world = session.task_world(Some(scope), &[])?;
        let task = wg_registry::resolve::task(&world.tasks, Some(scope), task_ref)?.clone();
        let mut work = Ingestion::start(session, &stamp);
        let id = work.id();
        let binding = SessionBinding {
            id,
            workspace_id: scope,
            task_id: task.id,
            provider: PROVIDER.to_owned(),
            external_session_id: session_id.map(str::to_owned),
            cwd_prefix: cwd.clone(),
            active: true,
            created_by: stamp.actor.to_string(),
            created_at: stamp.at,
        };
        work.put(binding.clone())?;
        if let Some(external) = session_id
            && let Some(existing) =
                sessions::agent_session_by_external(work.session, PROVIDER, external)?
        {
            work.update(ingest::bound_session(&existing, &task, stamp.at))?;
        }
        let payload = ingest::bound_event(session_id, cwd.as_deref());
        work.event(Some(scope), task.id, payload, None)?;
        Ok(json!({
            "binding_id": binding.id.to_string(),
            "task": task.key.to_string(),
            "session_id": session_id,
            "cwd": cwd,
        }))
    })
}

/// `str(Path(text).resolve())`: the absolute path, symlinks resolved when the path exists.
pub(crate) fn resolved_path(text: &str) -> String {
    let path = Path::new(text);
    let resolved = std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf());
    let shown = resolved.to_string_lossy().into_owned();
    shown
        .strip_prefix(r"\\?\")
        .map_or(shown.clone(), str::to_owned)
}

/// The files of a spool directory with an extension.
fn spool_files(dir: &Path, extension: &str) -> Vec<PathBuf> {
    std::fs::read_dir(dir).map_or_else(
        |_| Vec::new(),
        |entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == extension))
                .collect()
        },
    )
}

/// `ingest_direct`: opens the database of the home without migrating it and ingests the record.
/// Fails when the database does not exist or is not at the current schema, so the caller spools.
pub fn ingest_direct(home: Option<&Path>, record: &Value) -> Result<IngestResult, Failure> {
    let paths = resolve_paths(home)?;
    let database = PathBuf::from(paths.database_file().as_str());
    if !database.exists() {
        return Err(Failure::new(
            "RuntimeError",
            "Workgraph database not initialised",
        ));
    }
    let mut store = Store::open(&Location::File(database))?;
    if !store.is_up_to_date()? {
        let current = store.current_revision()?;
        return Err(Failure::new(
            "RuntimeError",
            format!(
                "database schema {} != expected {}",
                current.as_deref().unwrap_or("None"),
                wg_store::head_revision().unwrap_or("None")
            ),
        ));
    }
    let mut app = App {
        paths,
        runtime: Runtime {
            store,
            stamps: Box::new(SystemStamps),
            effects: Interpreter::default(),
        },
    };
    hook_ingest(&mut app, record, "direct")
}
