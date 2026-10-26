//! The automation feature's Pure spec (docs/rust-rewrite.md 4.16): the automation service as pure
//! decisions (`automation/engine.py`, `AutomationService`): rule
//! create, update and enable; the runs a manual trigger, a preview, an event pass and a schedule
//! pass record; the condition context a run is evaluated against; and the outputs.
//!
//! The shell loads what a decision reads (the rule, the event and the rows its context shows, the
//! counts of recent runs, the duplicate checks) and applies what the decision returns in one
//! transaction. Ids come from the stamp and times from `stamp.at`; nothing here reads a clock.

pub mod args;
pub mod dto;
pub mod execute;
pub mod shapes;
pub mod sql;
pub mod view;

use self::dto::automation as wire;
use self::dto::notification;
use rpds::VectorSync;
use serde_json::{Map, Value, json};
use wg_automation::condition::OPERATORS;
use wg_automation::guard;
use wg_automation::plan::{RunInput, RunPlan, plan_run};
use wg_automation::rules::{ACTION_TYPES, TRIGGER_EVENTS};
use wg_automation::spec::{Failure, Issue, RuleSpec, validate_rule};
use wg_jobs::queue::{NewJob, enqueue_as};
use wg_jobs::scheduler::next_time;
use wg_json::Value as WireValue;
use wg_model::entity::{
    AgentAttempt, AutomationRule, AutomationRun, Classification, Notification, SystemCursor, Task,
};
use wg_model::event::{AutomationRuleCreated, AutomationRuleToggled, AutomationRuleUpdated};
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::{Decision, EventDraft, EventPayload, Write};
use wg_storecodec::EventRecord;
use wg_types::{ErrorCode, Id, Timestamp, WgError};
use wg_wire::convention as wire_convention;

/// The name of the job that runs a queued run's actions.
pub const ACTION_JOB: &str = "automation_action";

/// The name of the cursor `process_events` keeps in `system_cursors`.
pub const CURSOR: &str = "automation";

/// `NotFoundError` for an event a preview names.
pub fn event_not_found(id: &str) -> WgError {
    WgError::new(ErrorCode::NotFound, format!("event {id} not found"))
}

/// The name a preview rule of `spec` carries.
pub fn preview_name(name: &str) -> String {
    format!("[preview] {name}")
}

/// `ValidationFailed("invalid automation rule", details={"errors": ...})`.
pub fn invalid_rule(issues: &[Issue]) -> WgError {
    WgError::new(ErrorCode::ValidationFailed, "invalid automation rule").with_detail(
        "errors",
        Value::Array(issues.iter().map(Issue::to_json).collect()),
    )
}

/// The `RuleOut` of a rule.
pub fn rule_out(rule: &AutomationRule) -> WireValue {
    wire::rule(rule)
}

/// The `RunOut` of a run.
pub fn run_out(run: &AutomationRun) -> WireValue {
    wire::run(run)
}

/// The `RuleOut` list of a listing.
pub fn rules_out(rules: &[AutomationRule]) -> WireValue {
    WireValue::List(rules.iter().map(rule_out).collect())
}

/// The `RunOut` list of a listing.
pub fn runs_out(runs: &[AutomationRun]) -> WireValue {
    WireValue::List(runs.iter().map(run_out).collect())
}

/// The result of `process`: the event pass's counts and the number of scheduled runs.
pub fn process_out(events: WireValue, scheduled: i64) -> WireValue {
    wire_convention::dict(vec![
        ("events", events),
        ("scheduled", wire_convention::int(scheduled)),
    ])
}

/// The notification list of a listing.
pub fn notifications_out(rows: &[Notification]) -> WireValue {
    WireValue::List(rows.iter().map(notification::from_model).collect())
}

/// The catalog: triggers, schedule keys, actions and operators.
pub fn catalog() -> WireValue {
    let operators: Vec<&str> = OPERATORS.iter().map(|op| op.name()).collect();
    wire::catalog(TRIGGER_EVENTS, &ACTION_TYPES, &operators)
}

/// The rule's text in the form the rule document stores it.
fn trigger_map(spec: &RuleSpec) -> Map<String, Value> {
    spec.trigger.clone()
}

/// `_check_privileged`: a rule that enqueues agent runs needs `run:start` to be created.
pub fn is_privileged(spec: &RuleSpec) -> bool {
    spec.actions.iter().any(|action| {
        action
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| wg_automation::rules::PRIVILEGED_ACTIONS.contains(&kind))
    })
}

/// A rule row from a validated spec. `max_depth` is capped by the configured maximum; a schedule
/// is first due one trigger interval after `at`.
fn rule_row(
    stamp: &Stamp,
    id: Id,
    workspace_id: Id,
    spec: &RuleSpec,
    config_max_depth: i64,
    version: i64,
    created_at: Timestamp,
) -> Result<AutomationRule, WgError> {
    let next_run_at = if spec.trigger_type() == "schedule" {
        Some(next_time(&spec.trigger, stamp.at)?)
    } else {
        None
    };
    Ok(AutomationRule {
        id,
        workspace_id,
        name: spec.name.clone(),
        description: spec.description.clone(),
        enabled: spec.enabled,
        trigger: trigger_map(spec),
        conditions: spec.conditions.clone(),
        actions: spec.actions.iter().cloned().collect::<VectorSync<_>>(),
        retry_max_attempts: spec.retry_max_attempts,
        max_depth: spec.max_depth.min(config_max_depth),
        last_scheduled_at: None,
        next_run_at,
        created_by: stamp.actor.to_string(),
        version,
        created_at,
        updated_at: stamp.at,
    })
}

/// `AutomationService.create_rule`: the new rule and its `automation.rule_created` event. `existing`
/// is the rule of the same name in the workspace, if there is one.
pub fn create_rule(
    stamp: &Stamp,
    workspace_id: Id,
    existing: Option<&AutomationRule>,
    spec: &RuleSpec,
    config_max_depth: i64,
) -> Result<Decision<AutomationRule>, WgError> {
    if existing.is_some() {
        return Err(WgError::new(
            ErrorCode::Conflict,
            format!("rule {} already exists", wg_model::py_repr_str(&spec.name)),
        ));
    }
    let row = rule_row(
        stamp,
        stamp.id(0),
        workspace_id,
        spec,
        config_max_depth,
        1,
        stamp.at,
    )?;
    let event = EventDraft::new(
        stamp,
        stamp.id(1),
        Some(workspace_id),
        row.id,
        EventPayload::AutomationRuleCreated(AutomationRuleCreated {
            name: row.name.clone(),
            trigger: Value::Object(row.trigger.clone()),
        }),
    );
    Ok(Decision::new(row.clone())
        .write(Write::insert(row))
        .event(event))
}

/// `AutomationService.update_rule`: the rule takes the spec, its version moves on and the last
/// editor becomes its author.
pub fn update_rule(
    stamp: &Stamp,
    existing: &AutomationRule,
    spec: &RuleSpec,
    config_max_depth: i64,
) -> Result<Decision<AutomationRule>, WgError> {
    let next_run_at = if spec.trigger_type() == "schedule" {
        Some(next_time(&spec.trigger, stamp.at)?)
    } else {
        existing.next_run_at
    };
    let row = AutomationRule {
        name: spec.name.clone(),
        description: spec.description.clone(),
        enabled: spec.enabled,
        trigger: trigger_map(spec),
        conditions: spec.conditions.clone(),
        actions: spec.actions.iter().cloned().collect(),
        retry_max_attempts: spec.retry_max_attempts,
        max_depth: spec.max_depth.min(config_max_depth),
        version: existing.version + 1,
        created_by: stamp.actor.to_string(),
        next_run_at,
        updated_at: stamp.at,
        ..existing.clone()
    };
    let event = EventDraft::new(
        stamp,
        stamp.id(0),
        Some(row.workspace_id),
        row.id,
        EventPayload::AutomationRuleUpdated(AutomationRuleUpdated {
            version: row.version,
        }),
    );
    Ok(Decision::new(row.clone())
        .write(Write::update(row))
        .event(event))
}

/// `AutomationService.set_enabled`: switches the rule on or off and records it.
pub fn set_enabled(
    stamp: &Stamp,
    rule: &AutomationRule,
    enabled: bool,
) -> Decision<AutomationRule> {
    let row = AutomationRule {
        enabled,
        updated_at: stamp.at,
        ..rule.clone()
    };
    let payload = if enabled {
        EventPayload::AutomationRuleEnabled(AutomationRuleToggled {})
    } else {
        EventPayload::AutomationRuleDisabled(AutomationRuleToggled {})
    };
    let event = EventDraft::new(stamp, stamp.id(0), Some(row.workspace_id), row.id, payload);
    Decision::new(row.clone())
        .write(Write::update(row))
        .event(event)
}

/// The row a preview of `spec` runs against: the `[preview] <name>` rule, created disabled on the
/// first preview and updated on later ones.
pub fn preview_target(
    stamp: &Stamp,
    workspace_id: Id,
    existing: Option<&AutomationRule>,
    spec: &RuleSpec,
) -> AutomationRule {
    match existing {
        Some(rule) => AutomationRule {
            enabled: false,
            trigger: trigger_map(spec),
            conditions: spec.conditions.clone(),
            actions: spec.actions.iter().cloned().collect(),
            updated_at: stamp.at,
            ..rule.clone()
        },
        None => AutomationRule {
            id: stamp.id(0),
            workspace_id,
            name: preview_name(&spec.name),
            description: String::new(),
            enabled: false,
            trigger: trigger_map(spec),
            conditions: spec.conditions.clone(),
            actions: spec.actions.iter().cloned().collect(),
            retry_max_attempts: 3,
            max_depth: 3,
            last_scheduled_at: None,
            next_run_at: None,
            created_by: stamp.actor.to_string(),
            version: 1,
            created_at: stamp.at,
            updated_at: stamp.at,
        },
    }
}

/// `_clean_inputs`: the scalar inputs with short keys, strings cut to 1000 characters.
pub fn clean_inputs(inputs: &Map<String, Value>) -> Map<String, Value> {
    inputs
        .iter()
        .filter(|(key, value)| {
            key.chars().count() <= 64
                && (value.is_string() || value.is_number() || value.is_boolean())
        })
        .map(|(key, value)| {
            let cleaned = match value {
                Value::String(text) => Value::String(text.chars().take(1000).collect()),
                other => other.clone(),
            };
            (key.clone(), cleaned)
        })
        .collect()
}

/// Everything `_context` shows of one trigger, as the shell loaded it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    pub event: Option<&'a EventRecord>,
    pub attempt: Option<&'a AgentAttempt>,
    pub task: Option<&'a Task>,
    /// The task's tags, sorted.
    pub tags: &'a [String],
    /// The task's current classification, when it has one.
    pub classification: Option<&'a Classification>,
}

/// `AutomationService._context`: the document conditions are evaluated against.
pub fn condition_context(at: Timestamp, extra: &Map<String, Value>, rows: Context<'_>) -> Value {
    let mut out = Map::new();
    out.insert("now".to_owned(), json!(at.format_isoformat()));
    out.insert("input".to_owned(), Value::Object(extra.clone()));
    if let Some(event) = rows.event {
        out.insert("event".to_owned(), event_value(event));
    }
    if let Some(attempt) = rows.attempt {
        out.insert("attempt".to_owned(), attempt_value(attempt));
    }
    if let Some(task) = rows.task {
        out.insert("task".to_owned(), task_value(task, rows.tags));
        if let Some(classification) = rows.classification {
            out.insert(
                "classification".to_owned(),
                classification_value(classification),
            );
        }
    }
    Value::Object(out)
}

fn event_value(event: &EventRecord) -> Value {
    json!({
        "type": event.event_type,
        "actor": event.actor,
        "actor_kind": event.actor_kind,
        "entity_type": event.entity_type,
        "entity_id": event.entity_id.to_string(),
        "data": Value::Object(event.data.clone()),
    })
}

fn attempt_value(attempt: &AgentAttempt) -> Value {
    json!({
        "id": attempt.id.to_string(),
        "status": attempt.status.as_str(),
        "provider": attempt.provider,
        "model": attempt.model,
        "number": attempt.number,
        "exit_reason": attempt.exit_reason,
    })
}

fn task_value(task: &Task, tags: &[String]) -> Value {
    json!({
        "id": task.id.to_string(),
        "key": task.key.to_string(),
        "title": task.title,
        "status": task.status.as_str(),
        "priority": task.priority.as_str(),
        "category": task.category,
        "tags": tags,
        "assignee": task.assignee,
        "auto_execute": task.auto_execute,
        "parent_id": task.parent_id.map(|id| id.to_string()),
        "description": task.description.chars().take(2000).collect::<String>(),
    })
}

fn classification_value(classification: &Classification) -> Value {
    json!({
        "category": classification.category,
        "confidence": classification.confidence.map(|real| real.0),
        "domain": classification.domain,
        "complexity": classification.complexity,
        "source": classification.source,
        "model": classification.model,
    })
}

/// One run to record: the rule and the trigger, the inputs and the state the guards read.
#[derive(Debug, Clone)]
pub struct RunRequest<'a> {
    pub rule: &'a AutomationRule,
    /// `event`, `schedule`, `manual` or `preview`.
    pub trigger_type: &'a str,
    pub trigger_key: String,
    pub event: Option<&'a EventRecord>,
    pub inputs: Map<String, Value>,
    pub dry_run: bool,
    /// A run of this rule with this trigger key exists already.
    pub exists: bool,
    /// Live, non-dry runs of the rule in the last minute.
    pub recent_runs: i64,
    pub config_max_depth: i64,
    pub context: Value,
}

/// A planned run: its row, its job when it is queued, and the outcome `_create_run` returns.
#[derive(Debug, Clone)]
pub struct PlannedRun {
    pub run: AutomationRun,
    pub job: Option<NewJob>,
    pub outcome: &'static str,
}

/// `_create_run`: plans one run. `None` when a run with the same trigger key exists (a live run is
/// never recorded twice).
pub fn plan_run_row(
    stamp: &Stamp,
    run_id: Id,
    request: &RunRequest<'_>,
) -> Result<Option<PlannedRun>, WgError> {
    if request.exists && !request.dry_run {
        return Ok(None);
    }
    let event_depth = request.event.map_or(0, |event| {
        guard::event_depth(&Value::Object(event.data.clone()))
    });
    let depth = guard::run_depth(event_depth);
    let actions: Vec<Map<String, Value>> = request.rule.actions.iter().cloned().collect();
    let plan: RunPlan = plan_run(&RunInput {
        depth,
        rule_max_depth: request.rule.max_depth,
        config_max_depth: request.config_max_depth,
        dry_run: request.dry_run,
        recent_runs: request.recent_runs,
        conditions: &request.rule.conditions,
        actions: &actions,
        context: &request.context,
    })
    .map_err(|message| WgError::new(ErrorCode::WorkgraphError, message))?;
    let outcome = plan.outcome(request.dry_run);
    let correlation = request.event.map_or_else(
        || stamp.correlation.clone(),
        |event| event.correlation_id.clone(),
    );
    let run = AutomationRun {
        id: run_id,
        rule_id: request.rule.id,
        workspace_id: request.rule.workspace_id,
        trigger_type: request.trigger_type.to_owned(),
        trigger_key: request.trigger_key.clone(),
        trigger_event_id: request.event.map(|event| event.id),
        depth,
        dry_run: request.dry_run,
        status: plan.status.to_owned(),
        matched: plan.matched,
        reason: plan.reason.clone(),
        condition_trace: objects(&plan.condition_trace),
        action_results: objects(&plan.action_results),
        error: None,
        attempts: 0,
        correlation_id: Some(correlation),
        created_at: stamp.at,
        finished_at: None,
        inputs: request.inputs.clone(),
    };
    let job = (plan.status == "queued").then(|| {
        let mut job = NewJob::new(ACTION_JOB);
        job.payload = Map::from_iter([("run_id".to_owned(), json!(run.id.to_string()))]);
        job.workspace_id = Some(request.rule.workspace_id);
        job.max_attempts = request.rule.retry_max_attempts;
        job.backoff_base_seconds = 5.0;
        job.idempotency_key = Some(format!("automation:{}", run.id));
        job.correlation_id = run.correlation_id.clone();
        job.causation_id = run.trigger_event_id;
        job
    });
    Ok(Some(PlannedRun { run, job, outcome }))
}

/// The JSON objects of a list of values (the non-objects are dropped; a plan only holds objects).
fn objects(values: &[Value]) -> VectorSync<Map<String, Value>> {
    values
        .iter()
        .filter_map(|value| value.as_object().cloned())
        .collect()
}

/// The writes that record a planned run: its row and, when it is queued, its job. `job_id` is the
/// id of the job, from the stamp, so each job of one decision gets its own.
pub fn run_changes(planned: &PlannedRun, stamp: &Stamp, job_id: Id) -> Decision<()> {
    let base = Decision::new(()).write(Write::insert(planned.run.clone()));
    match &planned.job {
        Some(job) => base.then(enqueue_as(&QueueSlice::default(), job, job_id, stamp).map(|_| ())),
        None => base,
    }
}

/// `AutomationService.trigger`: a manual trigger of an enabled rule. Agents may trigger only manual
/// rules. The reply is the run the trigger recorded.
pub fn trigger(
    stamp: &Stamp,
    request: &RunRequest<'_>,
    is_human: bool,
) -> Result<Decision<AutomationRun>, WgError> {
    if !request.rule.enabled {
        return Err(WgError::new(
            ErrorCode::Conflict,
            format!("rule {} is disabled", request.rule.name),
        ));
    }
    if request.rule.trigger.get("type").and_then(Value::as_str) != Some("manual") && !is_human {
        return Err(WgError::new(
            ErrorCode::PermissionDenied,
            "agents may only trigger rules whose trigger type is manual",
        ));
    }
    let planned = plan_run_row(stamp, stamp.id(0), request)?.ok_or_else(|| {
        WgError::new(
            ErrorCode::Conflict,
            "this manual trigger key was already used",
        )
    })?;
    let run = planned.run.clone();
    Ok(run_changes(&planned, stamp, stamp.id(1)).with_reply(run))
}

/// `AutomationService.preview` for a run: records a dry run and replies with it. `target` writes,
/// when there are any, go first in the same decision (the `[preview]` rule of a spec).
pub fn preview_run(
    stamp: &Stamp,
    target: Option<Write>,
    request: &RunRequest<'_>,
) -> Result<Decision<AutomationRun>, WgError> {
    let planned = plan_run_row(stamp, stamp.id(2), request)?
        .ok_or_else(|| WgError::new(ErrorCode::Conflict, "the dry run was already recorded"))?;
    let run = planned.run.clone();
    let base = match target {
        Some(write) => Decision::new(()).write(write),
        None => Decision::new(()),
    };
    Ok(base
        .then(run_changes(&planned, stamp, stamp.id(3)))
        .with_reply(run))
}

/// The slot text of a schedule (`next_run_at` in whole seconds, as Python's `isoformat()`).
pub fn slot_of(next: Timestamp) -> String {
    let moment = next.to_offset_datetime();
    let whole = moment.replace_nanosecond(0).unwrap_or(moment);
    Timestamp::from_offset_datetime(whole)
        .map(Timestamp::format_isoformat)
        .unwrap_or_else(|_| next.format_isoformat())
}

/// A rule the event pass considers, with the count of its live runs in the last minute.
#[derive(Debug, Clone, Copy)]
pub struct Counted<'a> {
    pub rule: &'a AutomationRule,
    pub recent_runs: i64,
}

/// An event the pass processes: the event, its context, and the rules that already ran for it.
#[derive(Debug, Clone)]
pub struct EventRow<'a> {
    pub event: &'a EventRecord,
    pub context: Value,
    pub ran_rules: Vec<Id>,
}

/// What one pass counted (`process_events`' result).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PassStats {
    pub events: i64,
    pub runs: i64,
    pub matched: i64,
    pub skipped: i64,
}

impl PassStats {
    /// The result dict of `process_events`.
    pub fn to_value(self) -> WireValue {
        wire_convention::dict(vec![
            ("events", wire_convention::int(self.events)),
            ("runs", wire_convention::int(self.runs)),
            ("matched", wire_convention::int(self.matched)),
            ("skipped", wire_convention::int(self.skipped)),
        ])
    }
}

/// The state the event pass carries from one event to the next.
struct Pass {
    decision: Decision<()>,
    stats: PassStats,
    /// The live runs each rule gained in this pass (they count against its rate limit).
    recent: Vec<(Id, i64)>,
    slot: u32,
}

impl Pass {
    fn recent_of(&self, rule: Id) -> i64 {
        self.recent
            .iter()
            .filter(|(id, _)| *id == rule)
            .map(|(_, count)| *count)
            .sum()
    }

    fn with_recent(self, rule: Id) -> Self {
        let recent = self
            .recent
            .into_iter()
            .chain(std::iter::once((rule, 1)))
            .collect();
        Pass { recent, ..self }
    }
}

/// `AutomationService.process_events`: the events after the cursor, each run against the enabled
/// event rules that its type and workspace match. The cursor moves to the last event.
pub fn process_events(
    stamp: &Stamp,
    cursor: Option<&SystemCursor>,
    rules: &[Counted<'_>],
    events: &[EventRow<'_>],
    config_max_depth: i64,
) -> Result<Decision<PassStats>, WgError> {
    let start = Pass {
        decision: Decision::new(()),
        stats: PassStats::default(),
        recent: Vec::new(),
        slot: 0,
    };
    let pass = events.iter().try_fold(start, |pass, row| {
        let counted_stats = PassStats {
            events: pass.stats.events + 1,
            ..pass.stats
        };
        let pass = Pass {
            stats: counted_stats,
            ..pass
        };
        rules
            .iter()
            .filter(|counted| {
                row.event.workspace_id == Some(counted.rule.workspace_id)
                    && event_rule_matches(counted.rule, &row.event.event_type)
            })
            .try_fold(pass, |pass, counted| -> Result<Pass, WgError> {
                if row.ran_rules.contains(&counted.rule.id) {
                    return Ok(pass);
                }
                let request = RunRequest {
                    rule: counted.rule,
                    trigger_type: "event",
                    trigger_key: row.event.id.to_string(),
                    event: Some(row.event),
                    inputs: Map::new(),
                    dry_run: false,
                    exists: false,
                    recent_runs: counted.recent_runs + pass.recent_of(counted.rule.id),
                    config_max_depth,
                    context: row.context.clone(),
                };
                let Some(planned) = plan_run_row(stamp, stamp.id(pass.slot), &request)? else {
                    return Ok(pass);
                };
                let changes = run_changes(&planned, stamp, stamp.id(pass.slot + 1));
                let queued = planned.run.status == "queued";
                let stats = PassStats {
                    runs: pass.stats.runs + 1,
                    matched: pass.stats.matched + i64::from(planned.outcome == "matched"),
                    skipped: pass.stats.skipped + i64::from(planned.outcome == "skipped"),
                    ..pass.stats
                };
                let next = Pass {
                    decision: pass.decision.then(changes),
                    stats,
                    recent: pass.recent,
                    slot: pass.slot + 2,
                };
                Ok(if queued {
                    next.with_recent(counted.rule.id)
                } else {
                    next
                })
            })
    })?;
    let last = events.last().map(|row| row.event.seq);
    let cursor_write = match (cursor, last) {
        (Some(cursor), Some(seq)) => Some(Write::update(SystemCursor {
            value: seq,
            updated_at: stamp.at,
            ..cursor.clone()
        })),
        (Some(_), None) => None,
        (None, seq) => Some(Write::insert(SystemCursor {
            name: CURSOR.to_owned(),
            value: seq.unwrap_or(0),
            updated_at: stamp.at,
        })),
    };
    let decision = match cursor_write {
        Some(write) => pass.decision.write(write),
        None => pass.decision,
    };
    Ok(decision.with_reply(pass.stats))
}

/// Whether an event rule subscribes to an event type.
fn event_rule_matches(rule: &AutomationRule, event_type: &str) -> bool {
    rule.trigger.get("type").and_then(Value::as_str) == Some("event")
        && rule
            .trigger
            .get("event_types")
            .and_then(Value::as_array)
            .is_some_and(|types| types.iter().any(|name| name.as_str() == Some(event_type)))
}

/// A schedule rule the pass considers, with the state of its next slot as the shell loaded it.
#[derive(Debug, Clone)]
pub struct ScheduleRow<'a> {
    pub rule: &'a AutomationRule,
    pub recent_runs: i64,
    /// Whether a run already exists for the rule's due slot (`schedule:<slot>`).
    pub slot_exists: bool,
}

/// `AutomationService.run_scheduled`: records a run for every schedule that is due, moves each
/// schedule on, and replies with how many runs it recorded.
pub fn run_scheduled(
    stamp: &Stamp,
    rows: &[ScheduleRow<'_>],
    config_max_depth: i64,
) -> Result<Decision<i64>, WgError> {
    let start = (Decision::new(()), 0_i64, 0_u32);
    let (decision, created, _) = rows
        .iter()
        .filter(|row| row.rule.trigger.get("type").and_then(Value::as_str) == Some("schedule"))
        .try_fold(
            start,
            |(decision, created, slot), row| -> Result<_, WgError> {
                let rule = row.rule;
                match rule.next_run_at {
                    None => {
                        let next = next_time(&rule.trigger, stamp.at)?;
                        let write = Write::update(AutomationRule {
                            next_run_at: Some(next),
                            ..rule.clone()
                        });
                        Ok((decision.write(write), created, slot))
                    }
                    Some(due) if due > stamp.at => Ok((decision, created, slot)),
                    Some(due) => {
                        let slot_text = slot_of(due);
                        let inputs = Map::from_iter([(
                            "scheduled_for".to_owned(),
                            Value::String(slot_text.clone()),
                        )]);
                        let request = RunRequest {
                            rule,
                            trigger_type: "schedule",
                            trigger_key: format!("schedule:{slot_text}"),
                            event: None,
                            inputs: inputs.clone(),
                            dry_run: false,
                            exists: row.slot_exists,
                            recent_runs: row.recent_runs,
                            config_max_depth,
                            context: condition_context(stamp.at, &inputs, Context::default()),
                        };
                        let planned = plan_run_row(stamp, stamp.id(slot), &request)?;
                        let next = next_time(&rule.trigger, stamp.at)?;
                        let moved = Write::update(AutomationRule {
                            last_scheduled_at: Some(stamp.at),
                            next_run_at: Some(next),
                            updated_at: stamp.at,
                            ..rule.clone()
                        });
                        let base = decision.write(moved);
                        match planned {
                            Some(planned) => Ok((
                                base.then(run_changes(&planned, stamp, stamp.id(slot + 1))),
                                created + 1,
                                slot + 2,
                            )),
                            None => Ok((base, created, slot + 2)),
                        }
                    }
                }
            },
        )?;
    Ok(decision.with_reply(created))
}

/// The rule of `name` in the workspace listing, if there is one (the create and update checks).
pub fn rule_named<'a>(rules: &'a [AutomationRule], name: &str) -> Option<&'a AutomationRule> {
    rules.iter().find(|rule| rule.name == name)
}

/// The rows a trigger's condition context shows, owned, as the shell loaded them.
#[derive(Debug, Clone, Default)]
pub struct TriggerRows {
    pub event: Option<EventRecord>,
    pub attempt: Option<AgentAttempt>,
    pub task: Option<Task>,
    /// The task's tags, sorted.
    pub tags: Vec<String>,
    pub classification: Option<Classification>,
}

impl TriggerRows {
    /// The rows as the context builder borrows them.
    pub fn context(&self) -> Context<'_> {
        Context {
            event: self.event.as_ref(),
            attempt: self.attempt.as_ref(),
            task: self.task.as_ref(),
            tags: &self.tags,
            classification: self.classification.as_ref(),
        }
    }
}

/// The `automation.max_depth` of the global configuration (3 when it is not set).
pub fn max_depth_of_config(config: &Map<String, Value>) -> i64 {
    config
        .get("automation")
        .and_then(|section| section.get("max_depth"))
        .and_then(Value::as_i64)
        .unwrap_or(3)
}

/// The rule document a request carries, validated (`_parse`).
pub fn parse_rule(spec: &Value) -> Result<RuleSpec, WgError> {
    validate_rule(spec).map_err(|failure| match failure {
        Failure::Invalid(issues) => invalid_rule(&issues),
        Failure::Raised(error) => error,
    })
}
