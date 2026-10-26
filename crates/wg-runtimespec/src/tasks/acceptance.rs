//! Acceptance: `accept`, `accept_manually`, `reject` and `promote_dependents`
//! (`VerificationService` in `verification/service.py` and `TaskService.promote_dependents`), as
//! decisions over the task world, plus the pieces a stack landing shares with them: recording an
//! acceptance (`_accept`) and releasing a child from its stack (`StackLander.release_child`).
//!
//! The decisions take what the shell read (the task world, the verification configurations, the
//! task's latest run, its latest succeeded attempt, whether the stack's integration is busy),
//! run the guards of `wg-verify` and `wg-stacks`, and return the writes and events. Accepting the
//! parent of a stack does not write anything here: the route says so and the shell lands the
//! stack (see [`super::landing`]).

use std::collections::BTreeMap;

use serde_json::{Value as Json, json};
use wg_jobs::queue::{NewJob, enqueue};
use wg_json::Value;
use wg_model::entity::{
    AgentAttempt, RevisionManifest, Stack, StackItem, TaskAcceptance, VerificationConfig,
    VerificationRun,
};
use wg_model::event::{
    EventDraft, EventPayload, StackItemReleased, StackItemStateChanged, StackParentRejected,
    TaskAccepted, TaskRejected,
};
use wg_model::slice::{QueueSlice, StackSlice};
use wg_model::stamp::Stamp;
use wg_model::{Decision, RowKey, Write};
use wg_registry::resolve;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::promote::{dependents, promote_dependents as promotable};
use wg_tasks::transition::{Applied, Change, refresh_one, transition};
use wg_types::{
    ErrorCode, Id, Permission, StackItemState, StackStatus, TaskKey, TaskStatus,
    VerificationStatus, WgError,
};
use wg_verify::{LatestRun, TaskScope, accept_refusal, applicable_checks, ran_fast_tier_only};
use wg_wire::convention::text;
use wg_wire::dto::verification::acceptance_from_model;

use super::writes::substamp;
use wg_stacks::item::{check_item_transition, satisfies_stack_dependency};
use wg_stacks::landing::{ACCEPTABLE_TASK_STATUSES, Dependent, guard_child, release_plan};
use wg_stacks::rebuild::{StackTask, dependents as stack_dependents};

/// The changes so far of a decision that spans several steps, and the stack rows as they stand
/// after them (the gate and the release read the stacks the earlier steps wrote).
#[derive(Debug, Clone)]
pub struct Run {
    pub applied: Applied,
    pub stacks: StackSlice,
}

impl Run {
    /// A run that starts from the world as loaded.
    pub fn start(world: &TaskWorld, stamp: &Stamp) -> Run {
        Run {
            applied: Applied {
                decision: Decision::new(()),
                ids: stamp.ids(),
                tasks: world.tasks.clone(),
            },
            stacks: world.stacks.clone(),
        }
    }

    /// The run after `applied`, which continued this run's decision: the writes it added are
    /// folded into the stack rows.
    fn advanced(&self, applied: Applied) -> Run {
        let seen = self.applied.decision.writes.len();
        let stacks = self
            .stacks
            .apply_all(applied.decision.writes.iter().skip(seen));
        Run { applied, stacks }
    }

    /// The run with one more decision's writes and events joined on.
    fn joined(&self, next: Decision<()>) -> Run {
        let stacks = self.stacks.apply_all(next.writes.iter());
        let tasks = self.applied.tasks.apply_all(next.writes.iter());
        Run {
            applied: Applied {
                decision: self.applied.decision.clone().then(next),
                ids: self.applied.ids,
                tasks,
            },
            stacks,
        }
    }
}

fn gone(task: Id) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("task {task} vanished from the slice"),
    )
}

/// The stack rebuild a release or deferral needs belongs to the stack integration service, which
/// this build does not have; the decision refuses instead of leaving the branch stale.
pub fn rebuild_unavailable(what: &str) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!(
            "{what} needs the stack's integration branches rebuilt, which this build cannot do yet"
        ),
    )
}

/// What the acceptance of one task reads besides the task world.
#[derive(Debug, Clone, Default)]
pub struct AcceptFacts {
    /// The run the acceptance is linked to.
    pub run: Option<VerificationRun>,
    /// The manifest of that run, which the accepted manifest copies.
    pub manifest: Option<RevisionManifest>,
    /// The task's latest succeeded attempt.
    pub attempt: Option<AgentAttempt>,
}

/// `StackLander.active_stack_of_parent`.
pub fn active_stack_of_parent(stacks: &StackSlice, task: Id) -> Option<&Stack> {
    stacks.active_stack_of_parent(task)
}

/// The task's item in its parent's active stack (`active_item`).
fn active_item(stacks: &StackSlice, task: Id) -> Option<(&Stack, &StackItem)> {
    stacks
        .stacks
        .values()
        .filter(|stack| !matches!(stack.status, StackStatus::Landed | StackStatus::Abandoned))
        .find_map(|stack| {
            stacks
                .items_of(stack.id)
                .find(|item| item.task_id == task)
                .map(|item| (stack, item))
        })
}

/// `VerificationService._accept`: records the acceptance (and the manifest it keeps), moves the
/// task to `accepted` and queues the change-surface job.
pub fn accept_in(
    run: &Run,
    stamp: &Stamp,
    task_id: Id,
    mode: &str,
    reason: &str,
    facts: &AcceptFacts,
) -> Result<(Run, TaskAcceptance), WgError> {
    let task = run
        .applied
        .tasks
        .task(task_id)
        .ok_or_else(|| gone(task_id))?;
    if !ACCEPTABLE_TASK_STATUSES.contains(&task.status) {
        return Err(WgError::new(
            ErrorCode::InvalidTransition,
            format!(
                "{} is {}; only completed work awaiting verification or review can be accepted",
                task.key, task.status
            ),
        ));
    }
    let ([manifest_id, acceptance_id, event_id], ids) = run.applied.ids.take::<3>();
    let attempt = facts.attempt.as_ref().map(|found| found.id);
    let manifest = facts
        .run
        .as_ref()
        .and_then(|found| found.manifest_id)
        .and(facts.manifest.as_ref())
        .map(|source| RevisionManifest {
            id: manifest_id,
            workspace_id: task.workspace_id,
            attempt_id: attempt,
            kind: "accepted".to_owned(),
            entries: source.entries.clone(),
            created_at: stamp.at,
        });
    let acceptance = TaskAcceptance {
        id: acceptance_id,
        task_id: task.id,
        attempt_id: attempt,
        verification_run_id: facts.run.as_ref().map(|found| found.id),
        decision: "accepted".to_owned(),
        mode: mode.to_owned(),
        reason: reason.to_owned(),
        actor: stamp.actor.to_string(),
        manifest_id: manifest.as_ref().map(|row| row.id),
        created_at: stamp.at,
    };
    let recorded = EventDraft::new(
        stamp,
        event_id,
        Some(task.workspace_id),
        task.id,
        EventPayload::TaskAccepted(TaskAccepted {
            mode: mode.to_owned(),
            reason: reason.to_owned(),
            verification_run_id: acceptance.verification_run_id,
            manifest_id: acceptance.manifest_id,
        }),
    )
    .with_attempt(attempt);
    let rows = manifest
        .into_iter()
        .fold(Decision::new(()), |decision, row| {
            decision.write(Write::insert(row))
        })
        .write(Write::insert(acceptance.clone()))
        .event(recorded);
    let recording = Run {
        applied: Applied {
            decision: run.applied.decision.clone().then(rows),
            ids,
            tasks: run.applied.tasks.clone(),
        },
        stacks: run.stacks.clone(),
    };
    let moved = transition(
        &recording.applied.tasks,
        &recording.stacks,
        stamp,
        recording.applied.ids,
        task.id,
        TaskStatus::Accepted,
        Change {
            reason: Some(&format!("{mode} acceptance")),
            privileged: true,
            caused_by: Some(event_id),
            ..Change::default()
        },
    )?;
    let queued = surface_job(
        stamp,
        task.workspace_id,
        task.id,
        acceptance_id,
        event_id,
        &moved,
    );
    let applied = Applied {
        decision: recording.applied.decision.then(moved.decision).then(queued),
        ids: moved.ids,
        tasks: moved.tasks,
    };
    Ok((run.advanced(applied), acceptance))
}

/// `_enqueue_surface`: the `compute_change_surface` job an acceptance queues. Its idempotency key
/// names the acceptance, so no stored job can already hold it.
fn surface_job(
    stamp: &Stamp,
    workspace: Id,
    task: Id,
    acceptance: Id,
    caused_by: Id,
    after: &Applied,
) -> Decision<()> {
    let mut job = NewJob::new("compute_change_surface");
    job.payload = json!({"task_id": task.to_string()})
        .as_object()
        .cloned()
        .unwrap_or_default();
    job.workspace_id = Some(workspace);
    job.task_id = Some(task);
    job.idempotency_key = Some(format!("surface:{task}:{acceptance}"));
    job.correlation_id = Some(stamp.correlation.clone());
    job.causation_id = Some(caused_by);
    let queued = enqueue(
        &QueueSlice::default(),
        &job,
        &substamp(stamp, after.ids.position()),
    );
    queued
        .writes
        .iter()
        .fold(Decision::new(()), |decision, write| {
            decision.write(write.clone())
        })
}

/// `json.dumps(detail, sort_keys=True)` for the objects and strings an item detail holds.
pub fn dump_sorted(value: &Json) -> String {
    match value {
        Json::Object(fields) => {
            let sorted: BTreeMap<&String, &Json> = fields.iter().collect();
            let members: Vec<String> = sorted
                .into_iter()
                .map(|(key, member)| format!("{}: {}", ascii(key), dump_sorted(member)))
                .collect();
            format!("{{{}}}", members.join(", "))
        }
        Json::String(text) => ascii(text),
        other => other.to_string(),
    }
}

/// A JSON string with every non-ASCII character escaped, as `ensure_ascii` writes it.
fn ascii(text: &str) -> String {
    let quoted = serde_json::to_string(text).unwrap_or_default();
    quoted
        .chars()
        .flat_map(|c| {
            let mut units = [0_u16; 2];
            if c.is_ascii() {
                vec![c.to_string()]
            } else {
                c.encode_utf16(&mut units)
                    .iter()
                    .map(|unit| format!("\\u{unit:04x}"))
                    .collect()
            }
        })
        .collect()
}

/// `StackVerifier.dependents`: the items of the stack that depend on `task`, directly or through
/// other items, by task number.
fn item_dependents(run: &Run, stack: &Stack, task: Id) -> Vec<Dependent> {
    let items: Vec<&StackItem> = run.stacks.items_of(stack.id).collect();
    let members: Vec<StackTask> = items
        .iter()
        .filter_map(|item| {
            run.applied.tasks.task(item.task_id).map(|found| StackTask {
                task_id: item.task_id.to_string(),
                number: found.number,
            })
        })
        .collect();
    let edges: Vec<(String, String)> = run
        .applied
        .tasks
        .dependencies
        .values()
        .map(|row| (row.task_id.to_string(), row.depends_on_id.to_string()))
        .collect();
    stack_dependents(&members, &edges, &task.to_string())
        .into_iter()
        .filter_map(|found| {
            items
                .iter()
                .find(|item| item.task_id.to_string() == found)
                .map(|item| Dependent {
                    item_id: item.id.to_string(),
                    task_key: run
                        .applied
                        .tasks
                        .task(item.task_id)
                        .map(|t| t.key.to_string())
                        .unwrap_or_default(),
                    state: item.state,
                })
        })
        .collect()
}

/// `StacksService.transition_item` to `pending` with `detail`: the item, the stack's update time,
/// the `stack.item_state_changed` event and, because the item leaves the integration-ready
/// states, the refresh of the tasks that wait on its task.
fn item_to_pending(
    run: &Run,
    stamp: &Stamp,
    stack: &Stack,
    parent_key: &TaskKey,
    item: &StackItem,
    detail: &str,
) -> Result<Run, WgError> {
    check_item_transition(item.state, StackItemState::Pending)?;
    let task = run
        .applied
        .tasks
        .task(item.task_id)
        .ok_or_else(|| gone(item.task_id))?;
    let (event_id, ids) = run.applied.ids.next();
    let moved = StackItem {
        state: StackItemState::Pending,
        detail: Some(detail.to_owned()),
        updated_at: stamp.at,
        ..item.clone()
    };
    let touched = Stack {
        updated_at: stamp.at,
        ..stack.clone()
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(stack.workspace_id),
        stack.id,
        EventPayload::StackItemStateChanged(StackItemStateChanged {
            task_key: task.key.clone(),
            parent_key: parent_key.clone(),
            from: item.state,
            to: StackItemState::Pending,
            detail: Some(detail.to_owned()),
            merge_order: item.merge_order,
        }),
    );
    let recorded = Run {
        applied: Applied {
            ids,
            ..run.applied.clone()
        },
        stacks: run.stacks.clone(),
    }
    .joined(
        Decision::new(())
            .write(Write::update(moved))
            .write(Write::update(touched))
            .event(event),
    );
    if !satisfies_stack_dependency(item.state) {
        return Ok(recorded);
    }
    let waiting: Vec<Id> = dependents(&recorded.applied.tasks, task.id)
        .into_iter()
        .map(|found| found.id)
        .collect();
    let refreshed = waiting
        .into_iter()
        .try_fold(recorded.applied.clone(), |so_far, dependent| {
            refresh_one(so_far, &recorded.stacks, stamp, dependent, event_id)
        })?;
    Ok(recorded.advanced(refreshed))
}

/// `StackLander.release_child`: an accepted child leaves its active stack. The stack to rebuild
/// is returned when the child's commits were on the integration branch.
pub fn release_child(run: &Run, stamp: &Stamp, task_id: Id) -> Result<(Run, Option<Id>), WgError> {
    let Some((stack, item)) = active_item(&run.stacks, task_id) else {
        return Ok((run.clone(), None));
    };
    let parent = run
        .applied
        .tasks
        .task(stack.parent_task_id)
        .ok_or_else(|| gone(stack.parent_task_id))?;
    let task = run
        .applied
        .tasks
        .task(task_id)
        .ok_or_else(|| gone(task_id))?;
    let plan = release_plan(
        &task.key.to_string(),
        &parent.key.to_string(),
        item.state,
        &item_dependents(run, stack, task_id),
    );
    let reset = plan
        .reset
        .iter()
        .try_fold(run.clone(), |so_far, dependent| {
            let found = so_far
                .stacks
                .items_of(stack.id)
                .find(|row| row.id.to_string() == dependent.item_id)
                .cloned();
            match found {
                Some(row) => item_to_pending(
                    &so_far,
                    stamp,
                    stack,
                    &parent.key,
                    &row,
                    &dump_sorted(&dependent.detail),
                ),
                None => Ok(so_far),
            }
        })?;
    let (event_id, ids) = reset.applied.ids.next();
    let released = EventDraft::new(
        stamp,
        event_id,
        Some(stack.workspace_id),
        stack.id,
        EventPayload::StackItemReleased(StackItemReleased {
            task_key: task.key.clone(),
            parent_key: parent.key.clone(),
            state: item.state,
            rebuild: plan.rebuild,
        }),
    );
    let touched = Stack {
        updated_at: stamp.at,
        ..stack.clone()
    };
    let done = Run {
        applied: Applied {
            ids,
            ..reset.applied.clone()
        },
        stacks: reset.stacks.clone(),
    }
    .joined(
        Decision::new(())
            .event(released)
            .write(Write::delete(RowKey::StackItem(item.id)))
            .write(Write::update(touched)),
    );
    Ok((done, plan.rebuild.then_some(stack.id)))
}

/// What `accept` reads: the task world and the facts of the task named by the reference.
#[derive(Debug, Clone, Default)]
pub struct AcceptWorld {
    pub world: TaskWorld,
    pub configs: Vec<VerificationConfig>,
    pub facts: AcceptFacts,
    /// Integration work for the task's stack is queued or running.
    pub busy: bool,
}

/// Where an `accept` goes.
#[derive(Debug, Clone)]
pub enum Route {
    /// The task is accepted; this is the `AcceptanceOut`.
    Done(Value),
    /// The task is the parent of an active stack: the shell lands that stack.
    Land { stack_id: Id },
}

fn scope_of(tasks: &wg_model::TaskSlice, task: &wg_model::Task) -> TaskScope {
    TaskScope {
        workspace_id: task.workspace_id,
        task_id: task.id,
        project_ids: tasks
            .projects_of(task.id)
            .map(|row| row.project_id)
            .collect(),
        repository_ids: tasks
            .repositories_of(task.id)
            .map(|row| row.repository_id)
            .collect(),
    }
}

/// `VerificationService.accept` (automated) and `accept_manually`.
pub fn accept_task(
    loaded: &AcceptWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    manual: bool,
    reason: &str,
) -> Result<Decision<Route>, WgError> {
    stamp.require(&[Permission::TaskAccept])?;
    if manual && reason.trim().is_empty() {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "manual acceptance requires a recorded reason",
        ));
    }
    let tasks = &loaded.world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    if let Some(item) = tasks.external_item_of(task.id) {
        return Err(WgError::new(
            ErrorCode::PermissionDenied,
            format!(
                "{} is tracked by {}; the tracker closes it when {} reaches done",
                task.key, item.connection, item.external_key
            ),
        )
        .with_detail("connection", item.connection.as_str().into())
        .with_detail("external_key", item.external_key.as_str().into()));
    }
    if let Some(stack) = loaded.world.stacks.active_stack_of_parent(task.id) {
        return Ok(Decision::new(Route::Land { stack_id: stack.id }));
    }
    let mode = if manual { "manual" } else { "automated" };
    let reason = if manual || !reason.is_empty() {
        reason
    } else {
        "required checks passed"
    };
    if !manual {
        let checks = applicable_checks(&loaded.configs, &scope_of(tasks, task))?;
        let latest = loaded.facts.run.as_ref().map(|found| LatestRun {
            run_id: found.id,
            status: VerificationStatus::parse(&found.status).unwrap_or(VerificationStatus::Pending),
            required_passed: found.required_passed,
            fast_tier_only: ran_fast_tier_only(
                &checks,
                &found.config_snapshot.iter().cloned().collect::<Vec<_>>(),
            ),
        });
        if let Some(refusal) = accept_refusal(checks.len(), latest) {
            return Err(refusal.to_error(&task.key.to_string()));
        }
    }
    if let Some((stack, _)) = active_item(&loaded.world.stacks, task.id) {
        guard_child(&task.key.to_string(), &stack.id.to_string(), loaded.busy)?;
    }
    let run = Run::start(&loaded.world, stamp);
    let (accepted, acceptance) = accept_in(&run, stamp, task.id, mode, reason, &loaded.facts)?;
    let (released, rebuild) = release_child(&accepted, stamp, task.id)?;
    if rebuild.is_some() {
        return Err(rebuild_unavailable("accepting a merged stack child"));
    }
    Ok(released
        .applied
        .decision
        .with_reply(Route::Done(acceptance_from_model(&acceptance))))
}

/// The mode and reason a stack landing records for an `accept` of the stack's parent.
pub fn land_request_of(manual: bool, reason: &str) -> (&'static str, String) {
    if manual {
        ("manual", reason.to_owned())
    } else if reason.is_empty() {
        ("automated", "required checks passed".to_owned())
    } else {
        ("automated", reason.to_owned())
    }
}

/// The `manual` flag and the reason of a validated `AcceptBody` or `RejectBody`.
pub fn body_parts(body: &Json) -> (bool, String) {
    (
        body.get("manual").and_then(Json::as_bool).unwrap_or(false),
        body.get("reason")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
    )
}

/// The active stack the task is an item of, whose integration its acceptance must not interrupt.
pub fn item_stack_id(stacks: &StackSlice, task: Id) -> Option<Id> {
    active_item(stacks, task).map(|(stack, _)| stack.id)
}

/// `VerificationService.reject`: the task goes back to `ready`, with a recorded decision.
pub fn reject_task(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    reason: &str,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskAccept])?;
    if reason.trim().is_empty() {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "rejection requires a reason",
        ));
    }
    let task = resolve::task(&world.tasks, workspace, reference)?;
    if !ACCEPTABLE_TASK_STATUSES.contains(&task.status) {
        return Err(WgError::new(
            ErrorCode::InvalidTransition,
            format!("{} is {}; nothing to reject", task.key, task.status),
        ));
    }
    let ([acceptance_id, event_id], ids) = stamp.ids().take::<2>();
    let acceptance = TaskAcceptance {
        id: acceptance_id,
        task_id: task.id,
        attempt_id: None,
        verification_run_id: None,
        decision: "rejected".to_owned(),
        mode: "manual".to_owned(),
        reason: reason.to_owned(),
        actor: stamp.actor.to_string(),
        manifest_id: None,
        created_at: stamp.at,
    };
    let rejected = EventDraft::new(
        stamp,
        event_id,
        Some(task.workspace_id),
        task.id,
        EventPayload::TaskRejected(TaskRejected {
            reason: reason.to_owned(),
        }),
    );
    let recorded = Decision::new(())
        .write(Write::insert(acceptance.clone()))
        .event(rejected);
    let moved = transition(
        &world.tasks,
        &world.stacks,
        stamp,
        ids,
        task.id,
        TaskStatus::Ready,
        Change {
            reason: Some(&format!("rejected: {reason}")),
            caused_by: Some(event_id),
            ..Change::default()
        },
    )?;
    let noted = match world.stacks.active_stack_of_parent(task.id) {
        Some(stack) => {
            let (stack_event, _) = moved.ids.next();
            Decision::new(()).event(EventDraft::new(
                stamp,
                stack_event,
                Some(stack.workspace_id),
                stack.id,
                EventPayload::StackParentRejected(StackParentRejected {
                    task_key: task.key.clone(),
                    reason: reason.to_owned(),
                }),
            ))
        }
        None => Decision::new(()),
    };
    Ok(recorded
        .then(moved.decision)
        .then(noted)
        .with_reply(acceptance_from_model(&acceptance)))
}

/// `TaskService.promote_dependents`: the backlog dependents of the task whose dependencies are
/// now all accepted go to `ready`; the reply is their keys.
pub fn promote_dependents(
    world: &TaskWorld,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let task = resolve::task(&world.tasks, workspace, reference)?;
    let moved = promotable(&world.tasks, &world.stacks, task.id);
    let done = moved.iter().try_fold(
        Applied {
            decision: Decision::new(()),
            ids: stamp.ids(),
            tasks: world.tasks.clone(),
        },
        |so_far, dependent| {
            let next = transition(
                &so_far.tasks,
                &world.stacks,
                stamp,
                so_far.ids,
                dependent.id,
                TaskStatus::Ready,
                Change {
                    reason: Some("dependencies accepted"),
                    ..Change::default()
                },
            )?;
            Ok::<_, WgError>(Applied {
                decision: so_far.decision.then(next.decision),
                ids: next.ids,
                tasks: next.tasks,
            })
        },
    )?;
    let keys = moved
        .iter()
        .map(|dependent| text(&dependent.key.to_string()))
        .collect();
    Ok(done.decision.with_reply(Value::List(keys)))
}

/// `VerificationService.acceptances`: the task's decisions, oldest first.
pub fn acceptances_out(rows: &[TaskAcceptance]) -> Value {
    let mut ordered: Vec<&TaskAcceptance> = rows.iter().collect();
    ordered.sort_by_key(|row| (row.created_at, row.id));
    Value::List(ordered.into_iter().map(acceptance_from_model).collect())
}

/// The latest succeeded attempt of each task a landing accepts.
pub type Attempts = BTreeMap<Id, AgentAttempt>;

/// `{**to_jsonable(out), "promoted": promoted}` for `accept --promote-dependents`: the model
/// as the plain dict `model_dump(mode="json")` makes of it (datetimes as `Z` text), with the
/// promoted keys appended.
pub fn with_promoted(out: &Value, promoted: &Value) -> Value {
    let plain = |value: &Value| match value {
        Value::DateTime(at) => Value::Str(wg_json::PyStr::from(at.format_z())),
        other => other.clone(),
    };
    match out {
        Value::Model { fields, .. } => Value::Dict(
            fields
                .iter()
                .map(|(name, value)| (wg_json::PyStr::from(name.as_str()), plain(value)))
                .chain(std::iter::once((
                    wg_json::PyStr::from("promoted"),
                    promoted.clone(),
                )))
                .collect(),
        ),
        other => other.clone(),
    }
}
