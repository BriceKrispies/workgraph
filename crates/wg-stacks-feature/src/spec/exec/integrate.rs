//! `StackIntegrator`: the transactions of the `stack_integrate` job around each merge.
//!
//! The job merges the due items of one repository one at a time. Each item takes two
//! transactions with the merge between them (git runs outside any transaction): [`begin_item`]
//! decides what to do with the item and [`finish_item`] records what the merge did.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use wg_model::event::{
    EventPayload, StackIntegrationConflict, StackIntegrationMerged, StackIntegrationSkipped,
};
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::{Decision, Task};
use wg_rows::task_world_sql::TaskWorld;
use wg_stacks::ejection::returns_to_ready;
use wg_stacks::merge::{
    ItemStep, MergeOutcome, conflict_comment, conflict_reason, conflict_record, fully_merged,
    merge_message, plan_item, record_merge, start_integrating,
};
use wg_stacks::queue::{DueItem, ItemDetail, is_settled, next_due};
use wg_tasks::transition::Change;
use wg_types::{Id, StackItemState, StackStatus, TaskStatus, WgError, parse_id};

use super::chain::Chain;
use super::text::dumps_sorted;
use super::worktree::IntegrationTarget;

/// The idempotency key of the `stack_integrate` job of a repository at a stack state.
#[must_use]
pub fn integration_key(stack_id: Id, repository_id: Id, version: Id) -> String {
    crate::spec::writes::integration_key(stack_id, repository_id, version)
}

/// The reason on a skipped item's event.
const SKIPPED_REASON: &str = "no change in this repository";

/// The reason on the status change that starts an integration.
const STARTED_REASON: &str = "integration started";

/// The item's detail as stored text (`dump_detail`).
#[must_use]
pub fn dump_detail(detail: &ItemDetail) -> String {
    dumps_sorted(&detail.to_value())
}

/// Microseconds since the epoch, which orders timestamps as the Python datetimes order.
fn micros(at: wg_types::Timestamp) -> i64 {
    i64::try_from(at.to_offset_datetime().unix_timestamp_nanos() / 1000).unwrap_or(i64::MAX)
}

/// `_settled`: no item is pending or ready, and at least one is on the integration branch.
#[must_use]
pub fn settled(world: &TaskWorld, stack_id: Id) -> bool {
    let states: Vec<StackItemState> = world
        .stacks
        .items_of(stack_id)
        .map(|item| item.state)
        .collect();
    is_settled(&states)
}

/// `_next_due`: the next ready item whose in-stack dependencies are merged.
pub fn due_item(world: &TaskWorld, stack_id: Id, processed: &[Id]) -> Result<Option<Id>, WgError> {
    let items: Vec<DueItem> = world
        .stacks
        .items_of(stack_id)
        .filter_map(|item| {
            world.tasks.task(item.task_id).map(|task| DueItem {
                id: item.id.to_string(),
                task_id: task.id.to_string(),
                state: item.state,
                priority: task.priority,
                updated_at: micros(item.updated_at),
                number: task.number,
            })
        })
        .collect();
    let edges: Vec<(String, String)> = world
        .tasks
        .dependencies
        .values()
        .map(|edge| (edge.task_id.to_string(), edge.depends_on_id.to_string()))
        .collect();
    let done: Vec<String> = processed.iter().map(ToString::to_string).collect();
    next_due(&items, &edges, &done)
        .map(|id| parse_id(&id))
        .transpose()
}

/// What `begin_item` found to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Begun {
    pub step: ItemStep,
    pub task_key: String,
    /// The merge commit message, for a merge.
    pub message: String,
}

/// The loaded item with its stack, task and parent.
struct Loaded {
    stack: wg_model::entity::Stack,
    item: wg_model::entity::StackItem,
    task: Task,
}

fn load_item(chain: &Chain<'_>, item_id: Id) -> Result<Loaded, WgError> {
    let item = chain.item(item_id)?.clone();
    let stack = chain.stack(item.stack_id)?.clone();
    let task = chain.task(item.task_id)?.clone();
    Ok(Loaded { stack, item, task })
}

/// The first transaction of `_integrate_item`. `heads` is the head the task left in each stack
/// repository it changed (`StackIntegrator.heads`).
pub fn begin_item(
    world: &TaskWorld,
    stamp: &Stamp,
    item_id: Id,
    target: &IntegrationTarget,
    heads: &BTreeMap<Id, String>,
) -> Result<Decision<Begun>, WgError> {
    let chain = Chain::start(world, stamp);
    let loaded = load_item(&chain, item_id)?;
    let detail = ItemDetail::load(loaded.item.detail.as_deref());
    let head = heads.get(&target.repository_id).map(String::as_str);
    let merged_head = detail
        .merges
        .get(&target.repository_name)
        .and_then(|merge| merge.get("head"))
        .and_then(Value::as_str);
    let step = plan_item(!heads.is_empty(), head, merged_head);
    let task_key = loaded.task.key.to_string();
    let message = merge_message(&task_key, &loaded.task.title, &target.integration_branch);
    let chain = match &step {
        ItemStep::MergeUnchanged => chain.item_to(
            loaded.stack.id,
            loaded.task.id,
            StackItemState::Merged,
            loaded.item.detail.clone(),
        )?,
        ItemStep::Skip => chain.event(
            loaded.stack.workspace_id,
            loaded.stack.id,
            EventPayload::StackIntegrationSkipped(StackIntegrationSkipped {
                task_key: loaded.task.key.clone(),
                repository: target.repository_name.clone(),
                reason: SKIPPED_REASON.to_owned(),
            }),
        ),
        ItemStep::Noop => chain,
        ItemStep::Merge(_) => match start_integrating(loaded.stack.status) {
            Some(next) => chain.stack_status(loaded.stack.id, next, STARTED_REASON),
            None => chain,
        },
    };
    Ok(chain.finish(Begun {
        step,
        task_key,
        message,
    }))
}

/// What the second transaction recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    Merged,
    Conflict,
}

/// The second transaction of `_integrate_item`: the merge's outcome becomes rows and events.
pub fn finish_item(
    world: &TaskWorld,
    stamp: &Stamp,
    item_id: Id,
    target: &IntegrationTarget,
    head: &str,
    outcome: &MergeOutcome,
    heads: &BTreeMap<Id, String>,
) -> Result<Decision<Done>, WgError> {
    let chain = Chain::start(world, stamp);
    let loaded = load_item(&chain, item_id)?;
    let detail = ItemDetail::load(loaded.item.detail.as_deref());
    if outcome.conflict_paths.is_some() {
        let (chain, _) = record_conflict(
            chain,
            item_id,
            &target.repository_name,
            &target.integration_branch,
            head,
            outcome,
            detail,
        )?;
        return Ok(chain.finish(Done::Conflict));
    }
    let detail = record_merge(
        &detail,
        &target.repository_name,
        head,
        &outcome.merge_commit,
    );
    let chain = chain.event(
        loaded.stack.workspace_id,
        loaded.stack.id,
        EventPayload::StackIntegrationMerged(StackIntegrationMerged {
            task_key: loaded.task.key.clone(),
            repository: target.repository_name.clone(),
            branch: target.integration_branch.clone(),
            head: head.to_owned(),
            merge_commit: outcome.merge_commit.clone(),
            already_integrated: outcome.already_integrated,
        }),
    );
    let named: Vec<(String, String)> = heads
        .iter()
        .filter_map(|(repository, sha)| {
            world
                .tasks
                .registry
                .repository(*repository)
                .map(|row| (row.name.clone(), sha.clone()))
        })
        .collect();
    let text = dump_detail(&detail);
    let chain = if fully_merged(&detail, &named) {
        chain.item_to(
            loaded.stack.id,
            loaded.task.id,
            StackItemState::Merged,
            Some(text),
        )?
    } else {
        chain.item_detail(item_id, text)
    };
    Ok(chain.finish(Done::Merged))
}

/// `record_conflict`: an item goes back to its agent after a merge into one repository
/// conflicted. The merge was aborted, so the branch did not move.
pub fn record_conflict<'a>(
    chain: Chain<'a>,
    item_id: Id,
    repository: &str,
    branch: &str,
    head: &str,
    outcome: &MergeOutcome,
    detail: ItemDetail,
) -> Result<(Chain<'a>, ItemDetail), WgError> {
    let loaded = load_item(&chain, item_id)?;
    let conflict = conflict_record(repository, branch, head, outcome);
    let detail = ItemDetail {
        conflicts: detail
            .conflicts
            .iter()
            .filter(|(name, _)| name.as_str() != repository)
            .map(|(name, value)| (name.clone(), value.clone()))
            .chain(std::iter::once((repository.to_owned(), conflict.clone())))
            .collect(),
        ..detail
    };
    let text = dump_detail(&detail);
    let event = conflict_event(&loaded.task, &conflict);
    let chain = chain
        .event(loaded.stack.workspace_id, loaded.stack.id, event)
        .item_detail(item_id, text.clone())
        .item_to(
            loaded.stack.id,
            loaded.task.id,
            StackItemState::Conflict,
            Some(text),
        )?;
    let status = chain.task(loaded.task.id)?.status;
    let chain = if returns_to_ready(status) {
        chain.task_to(
            loaded.task.id,
            TaskStatus::Ready,
            Change {
                reason: Some(&conflict_reason(repository)),
                ..Change::default()
            },
        )?
    } else {
        chain
    };
    let body = conflict_comment(&loaded.task.key.to_string(), branch, repository, &conflict);
    let data: Map<String, Value> = [("stack_item_conflict".to_owned(), conflict)]
        .into_iter()
        .collect();
    Ok((chain.comment(loaded.task.id, &body, &data, None)?, detail))
}

/// The `stack.integration_conflict` payload of a conflict record.
fn conflict_event(task: &Task, conflict: &Value) -> EventPayload {
    let text = |key: &str| conflict.get(key).and_then(Value::as_str).map(str::to_owned);
    EventPayload::StackIntegrationConflict(StackIntegrationConflict {
        task_key: task.key.clone(),
        repository: text("repository").unwrap_or_default(),
        head: text("head"),
        integration_tip: text("integration_tip"),
        paths: conflict
            .get("paths")
            .and_then(Value::as_array)
            .map(|paths| {
                paths
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        git_error: text("git_error"),
        instruction: text("instruction"),
    })
}

/// After a merge: the other repositories' `stack_integrate` jobs, which a merge can unblock
/// (dependents wait on it in another repository). `version` is the stack's newest event.
pub fn integrate_followup(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    exclude_repository: Id,
    version: Id,
    queue: &QueueSlice,
) -> Result<Decision<()>, WgError> {
    let chain = Chain::start(world, stamp);
    let active = world
        .stacks
        .stack(stack_id)
        .is_some_and(|stack| !wg_stacks::merge::is_inactive(stack.status));
    let chain = if active {
        chain.enqueue_integration(stack_id, Some(exclude_repository), version, queue)
    } else {
        chain
    };
    Ok(chain.finish(()))
}

/// The result object of an `integrate` that ran.
#[must_use]
pub fn ran_result(
    repository: &str,
    merged: &[String],
    skipped: &[String],
    conflicts: &[String],
) -> Value {
    json!({
        "status": "ran",
        "repository": repository,
        "merged": merged,
        "skipped": skipped,
        "conflicts": conflicts,
    })
}

/// The result object of an `integrate` for a stack that is no longer active.
#[must_use]
pub fn inactive_result() -> Value {
    json!({"status": "inactive", "merged": [], "skipped": [], "conflicts": []})
}

/// Whether a stack status counts as in progress for integration.
#[must_use]
pub fn in_progress(status: StackStatus) -> bool {
    !wg_stacks::merge::is_inactive(status)
}

/// The task of an item, for a caller that needs its key.
pub fn item_task(world: &TaskWorld, item_id: Id) -> Result<Task, WgError> {
    let item = world.stacks.items.get(&item_id).ok_or_else(|| {
        WgError::workgraph(format!("stack item {} vanished from the slice", item_id))
    })?;
    world
        .tasks
        .task(item.task_id)
        .cloned()
        .ok_or_else(|| WgError::workgraph(format!("task {} vanished from the slice", item.task_id)))
}
