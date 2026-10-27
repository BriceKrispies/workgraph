//! `StackVerifier`: the transactions of the `stack_verify` job, of a manual ejection and of the
//! rebuild of the integration branches.
//!
//! The choices (which check failed where, which prefix to probe, which item to eject) are
//! `wg_stacks`'; these functions turn each into the rows and events Python writes.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use wg_jobs::queue::{NewJob, enqueue};
use wg_model::entity::{StackItem, VerificationConfig};
use wg_model::event::{
    EventPayload, StackBisectProbe, StackItemEjected, StackParentNotReviewable, StackRebuilt,
    StackVerificationErrored, StackVerificationFailed, StackVerificationFailedOnBase,
    StackVerificationStarted, StackVerified,
};
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::{Decision, Task};
use wg_stacks::bisect::{Entry, sequence};
use wg_stacks::ejection::{
    RETURNABLE_TASK_STATUSES, bisected_comment, bisected_ejection, check_ejectable, manual_comment,
    manual_ejection, parked_detail, should_park_dependent,
};
use wg_stacks::merge::MergeOutcome;
use wg_stacks::queue::ItemDetail;
use wg_stacks::rebuild::{
    REBUILT_REASON, Replayed, StackTask, apply_replay, dependents, ejected_task_reason,
};
use wg_stacks::regression::Regressions;
use wg_stacks::verify::{
    Errored, StackRepoBase, in_scope, not_reviewable_message, review_parent_steps, verified_reason,
    verify_job_key, verify_resource_keys,
};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::transition::Change;
use wg_types::{
    ErrorCode, Id, Permission, StackItemState, StackStatus, TaskStatus, WgError, parse_id,
};
use wg_verify::{ApplicableCheck, TaskScope, applicable_checks};

use super::chain::Chain;
use super::integrate::{dump_detail, record_conflict, settled};
use super::text::dumps_sorted;
use super::worktree::IntegrationTarget;
use super::{item_rows, repos_of, stack_rows, str_list, str_map};
use crate::spec::active_stack;
use crate::spec::task_in;
use wg_tasks::accept::substamp;

fn parse(text: &str) -> Result<Id, WgError> {
    parse_id(text)
}

fn vanished(what: &str, id: Id) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("{what} {id} vanished from the slice"),
    )
}

/// `_entries`: the revision manifest of a verification, one entry per repository. `paths` names
/// the checkout each repository runs in; `branch` records the integration branch as the ref.
#[must_use]
pub fn manifest_entries(
    targets: &[IntegrationTarget],
    paths: &BTreeMap<String, String>,
    shas: &BTreeMap<String, String>,
    branch: bool,
) -> Vec<Map<String, Value>> {
    targets
        .iter()
        .map(|target| {
            let path = paths
                .get(&target.repository_name)
                .cloned()
                .unwrap_or_default();
            let entry: Vec<(&str, Value)> = vec![
                (
                    "repository_id",
                    Value::String(target.repository_id.to_string()),
                ),
                ("repository", Value::String(target.repository_name.clone())),
                ("checkout_path", Value::String(path)),
                (
                    "sha",
                    shas.get(&target.repository_name)
                        .map_or(Value::Null, |sha| Value::String(sha.clone())),
                ),
                (
                    "ref",
                    if branch {
                        Value::String(target.integration_branch.clone())
                    } else {
                        Value::Null
                    },
                ),
                ("access", Value::String("read_write".to_owned())),
            ];
            entry
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect()
        })
        .collect()
}

/// The `stack` member of the events of a tip run.
#[must_use]
pub fn tip_context(stack_id: Id, tips: &BTreeMap<String, String>) -> Value {
    serde_json::json!({"id": stack_id.to_string(), "tips": tips})
}

/// The `stack` member of the events of a probe.
#[must_use]
pub fn probe_context(stack_id: Id, position: usize, shas: &BTreeMap<String, String>) -> Value {
    serde_json::json!({"id": stack_id.to_string(), "position": position, "shas": shas})
}

// ---------------------------------------------------------------- start

/// What a verification job found at its start.
#[derive(Debug, Clone)]
pub enum Start {
    /// The stack landed, was abandoned or is gone.
    Inactive,
    /// An item is still pending or ready, or nothing has merged.
    Waiting,
    Go(Box<Plan>),
}

/// What a verification that goes ahead needs.
#[derive(Debug, Clone)]
pub struct Plan {
    pub parent_id: Id,
    pub parent_key: String,
    pub workspace_id: Id,
    pub repos: Vec<StackRepoBase>,
    /// The parent's applicable full checks that apply to the stack's repositories.
    pub checks: Vec<ApplicableCheck>,
}

/// The first read of `verify`.
pub fn verify_start(
    world: &TaskWorld,
    configs: &[VerificationConfig],
    stack_id: Id,
) -> Result<Start, WgError> {
    let Some(stack) = world.stacks.stack(stack_id) else {
        return Ok(Start::Inactive);
    };
    if wg_stacks::merge::is_inactive(stack.status) {
        return Ok(Start::Inactive);
    }
    if !settled(world, stack_id) {
        return Ok(Start::Waiting);
    }
    let parent = world
        .tasks
        .task(stack.parent_task_id)
        .ok_or_else(|| vanished("task", stack.parent_task_id))?;
    let repos = repos_of(world, stack_id);
    let scope = TaskScope {
        workspace_id: parent.workspace_id,
        task_id: parent.id,
        project_ids: world
            .tasks
            .projects_of(parent.id)
            .map(|row| row.project_id)
            .collect(),
        repository_ids: world
            .tasks
            .repositories_of(parent.id)
            .map(|row| row.repository_id)
            .collect(),
    };
    let checks = applicable_checks(configs, &scope)?
        .into_iter()
        .filter(|check| in_scope(&repos, &check.spec))
        .collect();
    Ok(Start::Go(Box::new(Plan {
        parent_id: parent.id,
        parent_key: parent.key.to_string(),
        workspace_id: stack.workspace_id,
        repos,
        checks,
    })))
}

/// Verification starts: the stack is `verifying` and the tips and checks are recorded.
pub fn verification_started(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    tips: &BTreeMap<String, String>,
    checks: &[String],
) -> Result<Decision<()>, WgError> {
    let chain = Chain::start(world, stamp);
    let workspace = chain.stack(stack_id)?.workspace_id;
    Ok(chain
        .stack_status(stack_id, StackStatus::Verifying, "verification started")
        .event(
            workspace,
            stack_id,
            EventPayload::StackVerificationStarted(StackVerificationStarted {
                tips: str_map(tips.iter()),
                checks: str_list(checks),
            }),
        )
        .finish(()))
}

// ---------------------------------------------------------------- the verification job

/// `StackVerifier._enqueue`: one `stack_verify` job keyed by the integration `tips` (and by
/// `key_version` for an on-demand request). The reply is the job's id, `None` for a stack that is
/// no longer active. `queue` holds the job already stored under the key, if any.
pub fn verification_job(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    tips: &[(String, String)],
    key_version: Option<Id>,
    causation: Id,
    queue: &QueueSlice,
) -> Decision<Option<Id>> {
    let Some(stack) = world
        .stacks
        .stack(stack_id)
        .filter(|stack| !wg_stacks::merge::is_inactive(stack.status))
    else {
        return Decision::new(None);
    };
    let repository_ids: Vec<String> = repos_of(world, stack_id)
        .into_iter()
        .map(|repo| repo.repository_id)
        .collect();
    let mut job = NewJob::new("stack_verify");
    job.payload = [("stack_id".to_owned(), Value::String(stack_id.to_string()))]
        .into_iter()
        .collect();
    job.workspace_id = Some(stack.workspace_id);
    job.idempotency_key = Some(verify_job_key(
        &stack_id.to_string(),
        tips,
        key_version.map(|id| id.to_string()).as_deref(),
    ));
    job.resource_keys = verify_resource_keys(&stack_id.to_string(), &repository_ids);
    job.causation_id = Some(causation);
    job.created_by = stamp.actor.to_string();
    enqueue(queue, &job, &substamp(stamp, 200)).map(Some)
}

/// The idempotency key `verification_job` queues under.
#[must_use]
pub fn verification_key(
    stack_id: Id,
    tips: &[(String, String)],
    key_version: Option<Id>,
) -> String {
    verify_job_key(
        &stack_id.to_string(),
        tips,
        key_version.map(|id| id.to_string()).as_deref(),
    )
}

// ---------------------------------------------------------------- errored

/// `_verification_errored`: a check could not judge its revision, so nothing passes and nothing
/// is ejected; the stack returns to `integrating`.
pub fn verification_errored(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    errored: &Errored,
) -> Result<Decision<()>, WgError> {
    let chain = Chain::start(world, stamp);
    let workspace = chain.stack(stack_id)?.workspace_id;
    let run_id = parse(errored.run_id.as_deref().unwrap_or_default())?;
    let reason = errored.reason();
    Ok(chain
        .event(
            workspace,
            stack_id,
            EventPayload::StackVerificationErrored(StackVerificationErrored {
                run_id,
                stage: errored.stage.as_str().to_owned(),
                checks: str_list(&errored.checks()),
                excerpt: str_map(errored.errors.iter().map(|(name, text)| (name, text))),
                reason: reason.clone(),
            }),
        )
        .stack_status(stack_id, StackStatus::Integrating, &reason)
        .finish(()))
}

// ---------------------------------------------------------------- pass

/// `_passed`: every merged item verifies, the verified children and the parent become
/// reviewable, and the stack is `verified`. The reply is the keys of the items verified now.
pub fn verification_passed(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    run_id: Option<&str>,
    tips: &BTreeMap<String, String>,
    reason: &str,
    tolerated: Option<&[(String, Vec<String>)]>,
) -> Result<Decision<Vec<String>>, WgError> {
    let chain = Chain::start(world, stamp);
    let stack = chain.stack(stack_id)?.clone();
    let parent = chain.task(stack.parent_task_id)?.clone();
    let run = run_id.map(parse).transpose()?;
    let merged: Vec<(StackItem, Task)> = item_rows(&chain.step.stacks, &chain.step.tasks, stack_id)
        .into_iter()
        .filter(|(item, _)| item.state == StackItemState::Merged)
        .collect();
    let verified: Vec<Task> = merged.iter().map(|(_, task)| task.clone()).collect();
    let chain = merged.iter().try_fold(chain, |chain, (item, task)| {
        chain.item_to(
            stack_id,
            task.id,
            StackItemState::Verified,
            item.detail.clone(),
        )
    })?;
    let why = verified_reason(&parent.key.to_string(), run_id);
    let waiting: Vec<Task> = item_rows(&chain.step.stacks, &chain.step.tasks, stack_id)
        .into_iter()
        .filter(|(item, task)| {
            item.state == StackItemState::Verified
                && task.status == TaskStatus::AwaitingVerification
        })
        .map(|(_, task)| task)
        .collect();
    let chain = waiting.iter().try_fold(chain, |chain, task| {
        chain.task_to(
            task.id,
            TaskStatus::AwaitingReview,
            Change {
                reason: Some(&why),
                run_id: Some(run),
                ..Change::default()
            },
        )
    })?;
    let chain = review_parent(chain, stack.id, parent.id, run_id, run)?;
    let tolerated_object: Option<Map<String, Value>> = tolerated.map(|found| {
        found
            .iter()
            .map(|(name, signatures)| (name.clone(), Value::from(signatures.clone())))
            .collect()
    });
    let chain = chain
        .stack_status(stack_id, StackStatus::Verified, reason)
        .event(
            stack.workspace_id,
            stack_id,
            EventPayload::StackVerified(StackVerified {
                run_id: run,
                tips: str_map(tips.iter()),
                tasks: verified.iter().map(|task| task.key.clone()).collect(),
                reason: reason.to_owned(),
                tolerated: tolerated_object,
            }),
        );
    Ok(chain.finish(verified.iter().map(|task| task.key.to_string()).collect()))
}

/// `_review_parent`: the parent walks to `awaiting_review` one legal step at a time. A parent
/// that cannot get there stays where it is and the reason is recorded.
fn review_parent<'a>(
    chain: Chain<'a>,
    stack_id: Id,
    parent_id: Id,
    run_text: Option<&str>,
    run: Option<Id>,
) -> Result<Chain<'a>, WgError> {
    let parent = chain.task(parent_id)?.clone();
    if parent.status == TaskStatus::AwaitingReview {
        return Ok(chain);
    }
    let why = verified_reason(&parent.key.to_string(), run_text);
    let steps = review_parent_steps(parent.status);
    walk(chain, steps, stack_id, parent_id, &why, run)
}

fn walk<'a>(
    chain: Chain<'a>,
    steps: &[TaskStatus],
    stack_id: Id,
    parent_id: Id,
    why: &str,
    run: Option<Id>,
) -> Result<Chain<'a>, WgError> {
    let change = Change {
        reason: Some(why),
        run_id: Some(run),
        ..Change::default()
    };
    match steps.split_first() {
        Some((step, rest)) => match chain.clone().task_to(parent_id, *step, change) {
            Ok(next) => walk(next, rest, stack_id, parent_id, why, run),
            Err(err) if err.code == ErrorCode::InvalidTransition => {
                not_reviewable(chain, stack_id, parent_id, &err.message)
            }
            Err(err) => Err(err),
        },
        None => {
            let parent = chain.task(parent_id)?.clone();
            if parent.status == TaskStatus::AwaitingReview {
                Ok(chain)
            } else {
                let message = not_reviewable_message(&parent.key.to_string(), parent.status);
                not_reviewable(chain, stack_id, parent_id, &message)
            }
        }
    }
}

fn not_reviewable<'a>(
    chain: Chain<'a>,
    stack_id: Id,
    parent_id: Id,
    message: &str,
) -> Result<Chain<'a>, WgError> {
    let parent = chain.task(parent_id)?.clone();
    let workspace = chain.stack(stack_id)?.workspace_id;
    Ok(chain.event(
        workspace,
        stack_id,
        EventPayload::StackParentNotReviewable(StackParentNotReviewable {
            task_key: parent.key.clone(),
            status: parent.status,
            reason: message.to_owned(),
        }),
    ))
}

// ---------------------------------------------------------------- failure

/// The merge sequence of the stack: the items whose merges are on the integration branch.
#[must_use]
pub fn merge_sequence(world: &TaskWorld, stack_id: Id) -> Vec<Entry> {
    sequence(&stack_rows(&world.stacks, &world.tasks, stack_id))
}

/// `_bisect`, first transaction: the tip's regressions are recorded. The reply is the merge
/// sequence.
pub fn verification_failed(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    run_id: &str,
    tips: &BTreeMap<String, String>,
    failing: &Regressions,
) -> Result<Decision<Vec<Entry>>, WgError> {
    let chain = Chain::start(world, stamp);
    let workspace = chain.stack(stack_id)?.workspace_id;
    let names = wg_stacks::regression::sorted_names(failing);
    let signatures: Map<String, Value> = failing
        .iter()
        .map(|(name, found)| (name.clone(), Value::from(found.clone())))
        .collect();
    Ok(chain
        .event(
            workspace,
            stack_id,
            EventPayload::StackVerificationFailed(StackVerificationFailed {
                run_id: parse(run_id)?,
                tips: str_map(tips.iter()),
                failed_checks: str_list(&names),
                signatures,
            }),
        )
        .finish(merge_sequence(world, stack_id)))
}

/// A failure that belongs to the base (or no merged change): nothing is ejected.
pub fn failed_on_base(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    run_id: Option<&str>,
    failed: &[String],
    reason: &str,
) -> Result<Decision<()>, WgError> {
    let chain = Chain::start(world, stamp);
    let workspace = chain.stack(stack_id)?.workspace_id;
    Ok(chain
        .event(
            workspace,
            stack_id,
            EventPayload::StackVerificationFailedOnBase(StackVerificationFailedOnBase {
                run_id: run_id.map(parse).transpose()?,
                failed_checks: str_list(failed),
                reason: reason.to_owned(),
            }),
        )
        .stack_status(stack_id, StackStatus::Integrating, reason)
        .finish(()))
}

/// One probe of the bisection.
#[allow(clippy::too_many_arguments)]
pub fn bisect_probe(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    position: usize,
    shas: &BTreeMap<String, String>,
    passed: bool,
    run_id: &str,
    failed_checks: &[String],
) -> Result<Decision<()>, WgError> {
    let chain = Chain::start(world, stamp);
    let workspace = chain.stack(stack_id)?.workspace_id;
    Ok(chain
        .event(
            workspace,
            stack_id,
            EventPayload::StackBisectProbe(StackBisectProbe {
                position: i64::try_from(position).unwrap_or(i64::MAX),
                shas: str_map(shas.iter()),
                passed,
                run_id: parse(run_id)?,
                failed_checks: str_list(failed_checks),
            }),
        )
        .finish(()))
}

// ---------------------------------------------------------------- ejection

/// What an ejection leaves on the child: the record, the comment and the task's reason.
struct Ejection {
    record: Value,
    comment: String,
    reason: String,
}

fn ejected_event(task: &Task, record: &Value) -> EventPayload {
    let text = |key: &str| record.get(key).and_then(Value::as_str).map(str::to_owned);
    EventPayload::StackItemEjected(StackItemEjected {
        task_key: task.key.clone(),
        failed_checks: record
            .get("failed_checks")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            }),
        manual: record.get("manual").and_then(Value::as_bool),
        position: record.get("position").and_then(Value::as_i64),
        reason: text("reason"),
        signatures: record.get("signatures").and_then(Value::as_object).cloned(),
        verification_run_id: text("verification_run_id").and_then(|id| parse(&id).ok()),
    })
}

/// `_apply_ejection`: the culprit's item is ejected, its task goes back to ready with the
/// comment, and the items that depend on it are parked as pending. The reply is the task key.
fn apply_ejection(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    task_id: Id,
    make: impl FnOnce(&str, &str) -> Ejection,
) -> Result<Decision<String>, WgError> {
    let chain = Chain::start(world, stamp);
    let stack = chain.stack(stack_id)?.clone();
    let parent = chain.task(stack.parent_task_id)?.clone();
    let task = chain.task(task_id)?.clone();
    let parent_key = parent.key.to_string();
    let task_key = task.key.to_string();
    let ejection = make(&parent_key, &task_key);
    let detail = ItemDetail {
        other: [("ejection".to_owned(), ejection.record.clone())]
            .into_iter()
            .collect(),
        ..ItemDetail::default()
    };
    let chain = chain
        .item_to(
            stack_id,
            task_id,
            StackItemState::Ejected,
            Some(dumps_sorted(&detail.to_value())),
        )?
        .event(
            stack.workspace_id,
            stack_id,
            ejected_event(&task, &ejection.record),
        );
    let status = chain.task(task_id)?.status;
    let reason = ejected_task_reason(&parent_key, &ejection.reason);
    let chain = if RETURNABLE_TASK_STATUSES.contains(&status) {
        chain.task_to(
            task_id,
            TaskStatus::Ready,
            Change {
                reason: Some(&reason),
                ..Change::default()
            },
        )?
    } else {
        chain
    };
    let data: Map<String, Value> = [("stack_item_ejection".to_owned(), ejection.record)]
        .into_iter()
        .collect();
    let chain = chain.comment(task_id, &ejection.comment, &data, None)?;
    let stack_tasks: Vec<StackTask> = item_rows(&chain.step.stacks, &chain.step.tasks, stack_id)
        .iter()
        .map(|(_, task)| StackTask {
            task_id: task.id.to_string(),
            number: task.number,
        })
        .collect();
    let edges: Vec<(String, String)> = chain
        .step
        .tasks
        .dependencies
        .values()
        .map(|edge| (edge.task_id.to_string(), edge.depends_on_id.to_string()))
        .collect();
    let parked = dependents(&stack_tasks, &edges, &task_id.to_string());
    let chain = parked.iter().try_fold(chain, |chain, dependent| {
        let dependent = parse(dependent)?;
        let state = chain
            .step
            .stacks
            .items_of(stack_id)
            .find(|item| item.task_id == dependent)
            .map(|item| item.state);
        match state {
            Some(state) if should_park_dependent(state) => chain.item_to(
                stack_id,
                dependent,
                StackItemState::Pending,
                Some(dumps_sorted(&parked_detail(&task_key))),
            ),
            _ => Ok(chain),
        }
    })?;
    Ok(chain.finish(task_key))
}

/// `_eject`: bisection placed the failure at `culprit`, the item at `position`.
pub fn eject_bisected(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    culprit: &Entry,
    position: usize,
    run_id: &str,
    signatures: &Regressions,
) -> Result<Decision<String>, WgError> {
    let task_id = parse(&culprit.task_id)?;
    let position = i64::try_from(position).unwrap_or(i64::MAX);
    apply_ejection(world, stamp, stack_id, task_id, |parent, task| Ejection {
        record: bisected_ejection(position, run_id, signatures),
        comment: bisected_comment(parent, task, position, run_id, signatures),
        reason: wg_stacks::rebuild::ejection_names(signatures),
    })
}

/// What a manual ejection found.
#[derive(Debug, Clone)]
pub struct EjectPlan {
    pub stack_id: Id,
    pub task_id: Id,
}

/// `StackVerifier.eject`, first transaction: the checks before anything changes.
pub fn eject_plan(
    world: &TaskWorld,
    stamp: &Stamp,
    scope: Id,
    parent_ref: &str,
    task_ref: &str,
    reason: &str,
) -> Result<EjectPlan, WgError> {
    stamp.require(&[Permission::StackWrite])?;
    manual_ejection(reason)?;
    let parent = task_in(world, scope, parent_ref)?;
    let child = task_in(world, scope, task_ref)?;
    let stack = active_stack(world, parent)?;
    let culprit = merge_sequence(world, stack.id)
        .into_iter()
        .find(|entry| entry.task_key == child.key.to_string());
    let (state, has_merges) = world
        .stacks
        .items_of(stack.id)
        .find(|item| item.task_id == child.id)
        .map_or((StackItemState::Pending, false), |item| {
            (
                item.state,
                !ItemDetail::load(item.detail.as_deref()).merges.is_empty(),
            )
        });
    check_ejectable(
        &child.key.to_string(),
        &parent.key.to_string(),
        state,
        culprit.is_some() && has_merges,
    )?;
    Ok(EjectPlan {
        stack_id: stack.id,
        task_id: child.id,
    })
}

/// A manual ejection: the child an operator named is ejected. The reply is its task key.
pub fn eject_manual(
    world: &TaskWorld,
    stamp: &Stamp,
    plan: &EjectPlan,
    reason: &str,
) -> Result<Decision<String>, WgError> {
    let record = manual_ejection(reason)?;
    let actor = stamp.actor.to_string();
    apply_ejection(world, stamp, plan.stack_id, plan.task_id, |parent, _| {
        Ejection {
            record,
            comment: manual_comment(parent, &actor, reason),
            reason: reason.to_owned(),
        }
    })
}

// ---------------------------------------------------------------- rebuild

/// One replayed merge, keyed by the item and repository it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayedMerge {
    pub item_id: Id,
    pub repository: String,
    pub head: String,
    pub outcome: MergeOutcome,
}

/// `rebuild`, second transaction: the replays become item details, conflicts are recorded and the
/// stack returns to `integrating`.
pub fn rebuild_apply(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    branches: &BTreeMap<String, String>,
    bases: &BTreeMap<String, String>,
    tips: &BTreeMap<String, String>,
    results: &[ReplayedMerge],
) -> Result<Decision<()>, WgError> {
    let chain = Chain::start(world, stamp);
    let workspace = chain.stack(stack_id)?.workspace_id;
    let entries = merge_sequence(world, stack_id);
    let (chain, replayed, conflicted) = entries.iter().try_fold(
        (chain, Vec::<String>::new(), Vec::<String>::new()),
        |(chain, replayed, conflicted), entry| -> Result<_, WgError> {
            let item_id = parse(&entry.item_id)?;
            let detail = ItemDetail::load(chain.item(item_id)?.detail.as_deref());
            let merges: Vec<Replayed> = entry
                .merges
                .keys()
                .map(|name| {
                    results
                        .iter()
                        .find(|found| found.item_id == item_id && found.repository == *name)
                        .map(|found| Replayed {
                            repository_name: name.clone(),
                            head: found.head.clone(),
                            outcome: found.outcome.clone(),
                        })
                        .ok_or_else(|| {
                            WgError::new(
                                ErrorCode::WorkgraphError,
                                format!("no replay of {} in {name}", entry.task_key),
                            )
                        })
                })
                .collect::<Result<_, _>>()?;
            let applied = apply_replay(detail, &merges);
            let chain = chain.item_detail(item_id, dump_detail(&applied.detail));
            let failures = applied.conflicts.clone();
            let replayed = if failures.is_empty() {
                replayed
                    .into_iter()
                    .chain(std::iter::once(entry.task_key.clone()))
                    .collect()
            } else {
                replayed
            };
            let conflicted: Vec<String> = conflicted
                .into_iter()
                .chain(failures.iter().map(|_| entry.task_key.clone()))
                .collect();
            let (chain, _) = failures.iter().try_fold(
                (chain, applied.detail),
                |(chain, detail), failed| -> Result<_, WgError> {
                    record_conflict(
                        chain,
                        item_id,
                        &failed.repository_name,
                        branches
                            .get(&failed.repository_name)
                            .map_or("", String::as_str),
                        &failed.head,
                        &failed.outcome,
                        detail,
                    )
                },
            )?;
            Ok((chain, replayed, conflicted))
        },
    )?;
    Ok(chain
        .stack_status(stack_id, StackStatus::Integrating, REBUILT_REASON)
        .event(
            workspace,
            stack_id,
            EventPayload::StackRebuilt(StackRebuilt {
                base_shas: str_map(bases.iter()),
                tips: str_map(tips.iter()),
                replayed: str_list(&replayed),
                conflicted: str_list(&conflicted),
            }),
        )
        .finish(()))
}
