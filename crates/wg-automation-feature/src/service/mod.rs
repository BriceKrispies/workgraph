//! The automation entry points of the application: the services behind `automation list, create,
//! update, show, enable, disable, trigger, preview, runs, notifications, catalog, process`.
//!
//! Each one checks its permission, loads the rows its decision reads in the transaction of the
//! call, hands them to the pure decision of [`crate::spec`] and applies what it returns. A rule
//! document is validated before anything is read, as the Python service validates its request
//! first. These are free functions over `&mut App` (and `&mut Session` in [`reads`]): the feature's
//! Shell, reached from the `cli`, `http` and `mcp` tables.

pub mod execute;
pub mod reads;

use serde_json::{Map, Value as Json};
use wg_jobs::shift;
use wg_json::Value;
use wg_model::entity::{
    AgentAttempt, AutomationRule, AutomationRun, Classification, Notification, Task,
};
use wg_model::{RowKey, Write};
use wg_registry::resolve;
use wg_runtime::{App, RequestContext};
use wg_store::{EventRecord, Session};
use wg_types::{ActorKind, Id, Permission, Timestamp, WgError, parse_id};

use crate::spec::sql::Page;
use crate::spec::{self as core, Counted, EventRow, PassStats, RunRequest, ScheduleRow};

/// Live runs are counted over this window for the rate limit (`timedelta(minutes=1)`).
const RATE_WINDOW_SECONDS: f64 = -60.0;

/// The most events one pass reads (`process_events`' batch).
const EVENT_BATCH: i64 = 500;

/// The most notifications a listing returns (`notifications`' default limit).
const NOTIFICATION_LIMIT: i64 = 50;

/// Checks a permission against a fresh stamp, before anything is looked up.
fn require(app: &mut App, context: &RequestContext, permission: Permission) -> Result<(), WgError> {
    app.runtime.stamps.stamp(context)?.require(&[permission])
}

/// The rule `reference` names in the workspace: by id when the id is that of a rule in the
/// workspace, else by name (`AutomationService._rule`).
fn find_rule(
    session: &mut Session<'_>,
    workspace: Id,
    reference: &str,
) -> Result<Option<AutomationRule>, WgError> {
    let by_id = match parse_id(reference) {
        Ok(id) => session
            .fetch::<AutomationRule>(&RowKey::AutomationRule(id))?
            .filter(|rule| rule.workspace_id == workspace),
        Err(_) => None,
    };
    match by_id {
        Some(rule) => Ok(Some(rule)),
        None => reads::rule_by_name(session, workspace, reference),
    }
}

/// [`find_rule`], or the `NotFound` the Python service raises.
fn rule_or_not_found(
    session: &mut Session<'_>,
    workspace: Id,
    reference: &str,
) -> Result<AutomationRule, WgError> {
    find_rule(session, workspace, reference)?.ok_or_else(|| {
        WgError::not_found(format!(
            "automation rule {} not found",
            wg_model::py_repr_str(reference)
        ))
    })
}

/// The task an event or an attempt names, looked up the way `_context` looks it up.
fn task_of_event(
    session: &mut Session<'_>,
    event: Option<&EventRecord>,
    attempt: Option<&AgentAttempt>,
) -> Result<Option<Task>, WgError> {
    let direct = match event {
        Some(record) if record.entity_type == "task" => Some(record.entity_id),
        Some(record) => record
            .data
            .get("task_id")
            .and_then(Json::as_str)
            .and_then(|text| parse_id(text).ok()),
        None => None,
    };
    let found = match direct {
        Some(id) => session.fetch::<Task>(&RowKey::Task(id))?,
        None => None,
    };
    match (found, attempt) {
        (None, Some(attempt)) => session.fetch::<Task>(&RowKey::Task(attempt.task_id)),
        (found, _) => Ok(found),
    }
}

/// `_context`'s rows for one trigger: the event, its attempt, its task (or the task the input
/// names), the task's tags and its classification.
pub(crate) fn load_rows(
    session: &mut Session<'_>,
    event: Option<EventRecord>,
    input: &Map<String, Json>,
) -> Result<core::TriggerRows, WgError> {
    let attempt = match &event {
        Some(record) if record.entity_type == "attempt" => {
            session.fetch::<AgentAttempt>(&RowKey::AgentAttempt(record.entity_id))?
        }
        _ => None,
    };
    let mut task = task_of_event(session, event.as_ref(), attempt.as_ref())?;
    if let (None, Some(reference)) = (&task, input.get("task").and_then(Json::as_str)) {
        let world = session.task_world(None, &[])?;
        if let Ok(id) = resolve::task(&world.tasks, None, reference).map(|task| task.id) {
            task = session.fetch::<Task>(&RowKey::Task(id))?;
        }
    }
    let (tags, classification) = match &task {
        Some(found) => {
            let world = session.task_world(Some(found.workspace_id), &[])?;
            let tags: Vec<String> = world.tasks.tags_of(found.id).map(str::to_owned).collect();
            let classification = match found.current_classification_id {
                Some(id) => session.fetch::<Classification>(&RowKey::Classification(id))?,
                None => None,
            };
            (tags, classification)
        }
        None => (Vec::new(), None),
    };
    Ok(core::TriggerRows {
        event,
        attempt,
        task,
        tags,
        classification,
    })
}

/// The live runs of `rule` created in the rate window that ends at `at`.
fn recent_runs(session: &mut Session<'_>, rule: Id, at: Timestamp) -> Result<i64, WgError> {
    reads::live_run_count(session, rule, shift(at, RATE_WINDOW_SECONDS))
}

/// The run recorded under `key`, read back from the store as Python's `session.scalar` reads it:
/// its JSON columns come back with their keys in stored (sorted) order.
fn reread_run(app: &mut App, key: &str) -> Result<AutomationRun, WgError> {
    app.runtime
        .store
        .read(|session| reads::run_by_trigger_key(session, key))?
        .ok_or_else(|| WgError::conflict(format!("run {key} was not recorded")))
}

/// `AutomationService.list_rules`.
pub fn list(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
) -> Result<Value, WgError> {
    require(app, context, Permission::AutomationRead)?;
    let ws = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| reads::rules(session, ws),
        |rules| Ok(core::rules_out(rules)),
    )
}

/// `AutomationService.get_rule`.
pub fn get(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    require(app, context, Permission::AutomationRead)?;
    let ws = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| rule_or_not_found(session, ws, reference),
        |rule| Ok(core::rule_out(rule)),
    )
}

/// `AutomationService.create_rule`: a rule from its specification.
pub fn create(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    spec: &Json,
) -> Result<Value, WgError> {
    require(app, context, Permission::AutomationManage)?;
    let rule_spec = core::parse_rule(spec)?;
    if core::is_privileged(&rule_spec) {
        require(app, context, Permission::RunStart)?;
    }
    let ws = app.workspace_or_default(workspace)?;
    let max_depth = core::max_depth_of_config(&app.config()?);
    let row = app
        .runtime
        .decide(
            context,
            |session| reads::rule_by_name(session, ws, &rule_spec.name),
            |existing, stamp| {
                core::create_rule(stamp, ws, existing.as_ref(), &rule_spec, max_depth)
            },
        )?
        .reply;
    Ok(core::rule_out(&row))
}

/// `AutomationService.update_rule`: replaces a rule's specification.
pub fn update(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    spec: &Json,
) -> Result<Value, WgError> {
    require(app, context, Permission::AutomationManage)?;
    let rule_spec = core::parse_rule(spec)?;
    if core::is_privileged(&rule_spec) {
        require(app, context, Permission::RunStart)?;
    }
    let ws = app.workspace_or_default(workspace)?;
    let max_depth = core::max_depth_of_config(&app.config()?);
    let row = app
        .runtime
        .decide(
            context,
            |session| rule_or_not_found(session, ws, reference),
            |existing, stamp| core::update_rule(stamp, existing, &rule_spec, max_depth),
        )?
        .reply;
    Ok(core::rule_out(&row))
}

/// `AutomationService.set_enabled`: switches a rule on or off.
pub fn set_enabled(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    enabled: bool,
) -> Result<Value, WgError> {
    require(app, context, Permission::AutomationManage)?;
    let ws = app.workspace_or_default(workspace)?;
    let row = app
        .runtime
        .decide(
            context,
            |session| rule_or_not_found(session, ws, reference),
            |rule, stamp| Ok(core::set_enabled(stamp, rule, enabled)),
        )?
        .reply;
    Ok(core::rule_out(&row))
}

/// `AutomationService.trigger`: a manual trigger, recorded as a run. `inputs` are the trigger's
/// inputs (the CLI's `--task` is one of them).
pub fn trigger(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    inputs: &Map<String, Json>,
) -> Result<Value, WgError> {
    require(app, context, Permission::AutomationTrigger)?;
    let ws = app.workspace_or_default(workspace)?;
    let max_depth = core::max_depth_of_config(&app.config()?);
    let clean = core::clean_inputs(inputs);
    let at = app.runtime.stamps.stamp(context)?.at;
    let is_human = context.actor.kind() == ActorKind::Human;
    let key = format!("manual:{}", app.runtime.stamps.stamp(context)?.id(0));
    app.runtime.decide(
        context,
        |session| {
            let rule = rule_or_not_found(session, ws, reference)?;
            let rows = load_rows(session, None, &clean)?;
            let recent = recent_runs(session, rule.id, at)?;
            Ok((rule, rows, recent))
        },
        |(rule, rows, recent), stamp| {
            let request = RunRequest {
                rule,
                trigger_type: "manual",
                trigger_key: key.clone(),
                event: None,
                inputs: clean.clone(),
                dry_run: false,
                exists: false,
                recent_runs: *recent,
                config_max_depth: max_depth,
                context: core::condition_context(stamp.at, &clean, rows.context()),
            };
            core::trigger(stamp, &request, is_human)
        },
    )?;
    let run = reread_run(app, &key)?;
    Ok(core::run_out(&run))
}

/// `AutomationService.preview`: a dry run of a stored rule, or of an unsaved rule document,
/// against an event or the inputs. A document is saved as its `[preview]` rule first.
pub fn preview(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: Option<&str>,
    spec: Option<&Json>,
    event_id: Option<&str>,
    inputs: &Map<String, Json>,
) -> Result<Value, WgError> {
    require(app, context, Permission::AutomationRead)?;
    let parsed = match (reference, spec) {
        (Some(_), _) => None,
        (None, Some(document)) => Some(core::parse_rule(document)?),
        (None, None) => {
            return Err(WgError::validation(
                "pass a rule name/id or a rule spec to preview",
            ));
        }
    };
    let ws = app.workspace_or_default(workspace)?;
    let max_depth = core::max_depth_of_config(&app.config()?);
    let clean = core::clean_inputs(inputs);
    let key = format!("dry:{}", app.runtime.stamps.stamp(context)?.id(4));
    app.runtime.decide(
        context,
        |session| {
            let target = match (reference, &parsed) {
                (Some(name), _) => Some(rule_or_not_found(session, ws, name)?),
                (None, Some(document)) => {
                    reads::rule_by_name(session, ws, &core::preview_name(&document.name))?
                }
                (None, None) => None,
            };
            let event = match event_id {
                Some(id) => {
                    let found = match parse_id(id) {
                        Ok(parsed_id) => reads::event(session, parsed_id)?,
                        Err(_) => None,
                    };
                    Some(found.ok_or_else(|| core::event_not_found(id))?)
                }
                None => None,
            };
            let rows = load_rows(session, event, &clean)?;
            Ok((target, rows))
        },
        |(target, rows), stamp| {
            let (rule, write) = match (&parsed, target) {
                (Some(document), existing) => {
                    let row = core::preview_target(stamp, ws, existing.as_ref(), document);
                    let write = match existing {
                        Some(_) => Write::update(row.clone()),
                        None => Write::insert(row.clone()),
                    };
                    (row, Some(write))
                }
                (None, Some(stored)) => (stored.clone(), None),
                (None, None) => {
                    return Err(WgError::validation(
                        "pass a rule name/id or a rule spec to preview",
                    ));
                }
            };
            let request = RunRequest {
                rule: &rule,
                trigger_type: "preview",
                trigger_key: key.clone(),
                event: rows.event.as_ref(),
                inputs: clean.clone(),
                dry_run: true,
                exists: false,
                // Dry runs are never rate limited, so no count is read for them.
                recent_runs: 0,
                config_max_depth: max_depth,
                context: core::condition_context(stamp.at, &clean, rows.context()),
            };
            core::preview_run(stamp, write, &request)
        },
    )?;
    let run = reread_run(app, &key)?;
    Ok(core::run_out(&run))
}

/// `AutomationService.runs`: the trigger and execution history, newest first.
pub fn runs(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    rule: Option<&str>,
    status: Option<&str>,
    limit: i64,
) -> Result<Value, WgError> {
    require(app, context, Permission::AutomationRead)?;
    let ws = app.workspace_or_default(workspace)?;
    let rule = rule.filter(|name| !name.is_empty());
    let status = status.filter(|text| !text.is_empty());
    app.runtime.query(
        |session| {
            let rule_id = match rule {
                Some(reference) => Some(rule_or_not_found(session, ws, reference)?.id),
                None => None,
            };
            reads::runs(session, ws, rule_id, status, Page::first(limit))
        },
        |runs| Ok(core::runs_out(runs)),
    )
}

/// `AutomationService.notifications`: the local notifications rules emitted, newest first.
pub fn notifications(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    unread_only: bool,
) -> Result<Value, WgError> {
    require(app, context, Permission::WorkspaceRead)?;
    let ws = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| reads::notifications(session, ws, unread_only, Page::first(NOTIFICATION_LIMIT)),
        |rows| Ok(core::notifications_out(rows)),
    )
}

/// `NotificationService.mark_read`: stamps a notification read now. No permission is checked
/// (the Python service checks none), so the route is open to any caller.
pub fn mark_read(
    app: &mut App,
    context: &RequestContext,
    notification_id: &str,
) -> Result<(), WgError> {
    let id = parse_id(notification_id).ok();
    app.runtime
        .decide(
            context,
            |session| match id {
                Some(found) => session.fetch::<Notification>(&RowKey::Notification(found)),
                None => Ok(None),
            },
            |found, stamp| core::routes::mark_read(found.clone(), stamp),
        )
        .map(|_| ())
}

/// `process`: `AutomationService.process_events` then `run_scheduled`, run now. The reply is
/// `{"events": stats, "scheduled": n}`.
pub fn process(app: &mut App, context: &RequestContext) -> Result<Value, WgError> {
    let events = events_pass(app, context)?;
    let scheduled = scheduled_pass(app, context)?;
    Ok(core::process_out(events, scheduled))
}

/// `AutomationService.process_events`: the events after the cursor, through the enabled event
/// rules. Everything a pass reads is loaded here; the decision applies the writes.
pub(crate) fn events_pass(app: &mut App, context: &RequestContext) -> Result<Value, WgError> {
    let at = app.runtime.stamps.stamp(context)?.at;
    let max_depth = core::max_depth_of_config(&app.config()?);
    let (cursor, counted, events) = app.runtime.store.read(|session| {
        let cursor = session.system_cursor(core::CURSOR)?;
        let after = cursor.as_ref().map_or(0, |found| found.value);
        let mut counted = Vec::new();
        for rule in reads::enabled_rules(session, "event")? {
            let recent = recent_runs(session, rule.id, at)?;
            counted.push((rule, recent));
        }
        let mut events = Vec::new();
        for record in reads::events_after(session, after, EVENT_BATCH)? {
            let mut ran = Vec::new();
            for (rule, _) in &counted {
                let key = record.id.to_string();
                if reads::run_by_trigger(session, rule.id, &key)?.is_some() {
                    ran.push(rule.id);
                }
            }
            let rows = load_rows(session, Some(record.clone()), &Map::new())?;
            events.push((record, rows, ran));
        }
        Ok((cursor, counted, events))
    })?;
    let rules: Vec<Counted<'_>> = counted
        .iter()
        .map(|(rule, recent)| Counted {
            rule,
            recent_runs: *recent,
        })
        .collect();
    let rows: Vec<EventRow<'_>> = events
        .iter()
        .map(|(record, loaded, ran)| EventRow {
            event: record,
            context: core::condition_context(at, &Map::new(), loaded.context()),
            ran_rules: ran.clone(),
        })
        .collect();
    let stats: PassStats = app
        .runtime
        .decide(
            context,
            |_| Ok(()),
            |_, stamp| core::process_events(stamp, cursor.as_ref(), &rules, &rows, max_depth),
        )?
        .reply;
    Ok(stats.to_value())
}

/// `AutomationService.run_scheduled`: the schedules that are due, recorded as runs. Returns how
/// many runs were recorded.
pub(crate) fn scheduled_pass(app: &mut App, context: &RequestContext) -> Result<i64, WgError> {
    let at = app.runtime.stamps.stamp(context)?.at;
    let max_depth = core::max_depth_of_config(&app.config()?);
    let loaded = app.runtime.store.read(|session| {
        let mut rows = Vec::new();
        for rule in reads::enabled_rules(session, "schedule")? {
            let recent = recent_runs(session, rule.id, at)?;
            let slot_exists = match rule.next_run_at {
                Some(next) => reads::run_by_trigger(
                    session,
                    rule.id,
                    &format!("schedule:{}", core::slot_of(next)),
                )?
                .is_some(),
                None => false,
            };
            rows.push((rule, recent, slot_exists));
        }
        Ok(rows)
    })?;
    let rows: Vec<ScheduleRow<'_>> = loaded
        .iter()
        .map(|(rule, recent, slot_exists)| ScheduleRow {
            rule,
            recent_runs: *recent,
            slot_exists: *slot_exists,
        })
        .collect();
    app.runtime
        .decide(
            context,
            |_| Ok(()),
            |_, stamp| core::run_scheduled(stamp, &rows, max_depth),
        )
        .map(|outcome| outcome.reply)
}
