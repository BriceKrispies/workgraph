//! Stack landing, the decisions the shell applies around its git calls (`StackLander` in
//! `integrations/git/stack_landing.py`): the state `_land` checks, the writes of `_finish`, the
//! refusal event, and the removal of a stack's integration worktrees and branches.
//!
//! The shell reads the rows and the git facts, asks [`land_state`] for the preconditions, moves
//! the default branches with the plan `wg_stacks::landing` makes, and then applies
//! [`finish_landing`] in one transaction.

use std::collections::BTreeMap;

use rpds::VectorSync;
use wg_json::Value;
use wg_model::entity::{
    ManagedWorktree, Repository, RepositoryCheckout, RevisionManifest, Stack, StackItem,
    StackRepository, VerificationRun,
};
use wg_model::event::{
    EventDraft, EventPayload, StackIntegrationRemoved, StackLanded, StackLandingRefused,
    StackStatusChanged, WorktreeRemoved,
};
use wg_model::stamp::Stamp;
use wg_model::{Decision, StrMap, Write};
use wg_registry::checkout::primary_checkout;
use wg_stacks::landing::{
    FfMove, FfStep, JobRef, LandItem, LandRepo, LandState, child_accept_reason, integration_busy,
    landed_reason, unverified_children,
};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::transition::{Change, transition};
use wg_types::{
    CommentKind, ErrorCode, Id, StackItemState, StackStatus, TaskKey, TaskStatus, WgError,
    WorktreeStatus,
};
use wg_wire::dto::verification::acceptance_from_model;

use super::acceptance::{AcceptFacts, Attempts, Run, accept_in, release_child};
use super::claims::comment_step;

/// What the landing of one stack reads from the rows, per repository of the stack.
#[derive(Debug, Clone)]
pub struct RepoRows {
    pub link: StackRepository,
    pub repository: Repository,
    pub checkouts: Vec<RepositoryCheckout>,
}

/// An integration target (`IntegrationTarget`): where one repository of the stack integrates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub repository_id: Id,
    pub repository_name: String,
    pub default_branch: Option<String>,
    pub integration_branch: String,
    pub base_sha: String,
    pub source_path: String,
}

/// The targets of the stack's repositories, by repository name. A repository with no available
/// checkout is a git error (`target`).
pub fn targets(rows: &[RepoRows]) -> Result<Vec<Target>, WgError> {
    let mut found = rows
        .iter()
        .map(|row| {
            let checkout = primary_checkout(&row.checkouts, true).ok_or_else(|| {
                WgError::new(
                    ErrorCode::GitError,
                    format!(
                        "repository {} has no available checkout",
                        row.repository.name
                    ),
                )
            })?;
            Ok(Target {
                repository_id: row.repository.id,
                repository_name: row.repository.name.clone(),
                default_branch: row.repository.default_branch.clone(),
                integration_branch: row.link.integration_branch.clone(),
                base_sha: row.link.base_sha.clone(),
                source_path: checkout.path.clone(),
            })
        })
        .collect::<Result<Vec<_>, WgError>>()?;
    found.sort_by(|a, b| a.repository_name.cmp(&b.repository_name));
    Ok(found)
}

/// What the shell resolved with git for each target, in target order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heads {
    /// Where the integration branch is now.
    pub integrated: String,
    /// Where the default branch is now.
    pub default: String,
}

/// The facts `_land` checks, in the order the checks run. `heads` has one entry per target.
pub fn land_state(
    world: &TaskWorld,
    stack: &Stack,
    verified: bool,
    tips: &BTreeMap<String, String>,
    targets: &[Target],
    heads: &[Heads],
) -> Result<LandState, WgError> {
    let parent = world
        .tasks
        .task(stack.parent_task_id)
        .ok_or_else(|| WgError::new(ErrorCode::NotFound, "the stack's parent task is gone"))?;
    let items = world
        .stacks
        .items_of(stack.id)
        .filter_map(|item| {
            world.tasks.task(item.task_id).map(|task| LandItem {
                key: task.key.to_string(),
                number: task.number,
                merge_order: item.merge_order,
                state: item.state,
                task_status: task.status,
                detail: item.detail.clone(),
            })
        })
        .collect();
    let repos = targets
        .iter()
        .zip(heads)
        .map(|(target, head)| LandRepo {
            id: target.repository_id.to_string(),
            name: target.repository_name.clone(),
            default_branch: target.default_branch.clone(),
            base_sha: target.base_sha.clone(),
            integrated_tip: head.integrated.clone(),
            default_head: head.default.clone(),
        })
        .collect();
    Ok(LandState {
        parent_key: parent.key.to_string(),
        stack_id: stack.id.to_string(),
        stack_status: stack.status,
        parent_status: parent.status,
        items,
        verified_event: verified,
        repos,
        tips: tips.clone(),
    })
}

/// `land` on a refusal: the `stack.landing_refused` event, recorded before the error is raised.
pub fn refusal_event(
    stamp: &Stamp,
    stack: &Stack,
    mode: &str,
    reason: &str,
    error: &WgError,
) -> Decision<()> {
    let (id, _) = stamp.ids().next();
    Decision::new(()).event(EventDraft::new(
        stamp,
        id,
        Some(stack.workspace_id),
        stack.id,
        EventPayload::StackLandingRefused(StackLandingRefused {
            mode: mode.to_owned(),
            reason: reason.to_owned(),
            error: error.message.clone(),
        }),
    ))
}

/// What `_finish` reads.
#[derive(Debug, Clone)]
pub struct LandWorld {
    pub world: TaskWorld,
    pub stack_id: Id,
    /// The run id the stack's last `stack.verified` event names.
    pub run_id: Option<Id>,
    pub run: Option<VerificationRun>,
    pub manifest: Option<RevisionManifest>,
    /// The latest succeeded attempt of each task of the stack and of its parent.
    pub attempts: Attempts,
}

fn missing(what: &str) -> WgError {
    WgError::new(ErrorCode::NotFound, format!("{what} not found"))
}

/// The stack's items with their task numbers, in `item_rows` order: merge order (unset last),
/// then task number.
fn item_rows(run: &Run, stack: Id) -> Vec<StackItem> {
    let mut rows: Vec<(bool, i64, i64, StackItem)> = run
        .stacks
        .items_of(stack)
        .map(|item| {
            let number = run
                .applied
                .tasks
                .task(item.task_id)
                .map_or(i64::MAX, |task| task.number);
            (
                item.merge_order.is_none(),
                item.merge_order.unwrap_or(0),
                number,
                item.clone(),
            )
        })
        .collect();
    rows.sort_by_key(|row| (row.0, row.1, row.2));
    rows.into_iter().map(|row| row.3).collect()
}

fn facts_of(loaded: &LandWorld, task: Id) -> AcceptFacts {
    AcceptFacts {
        run: loaded.run.clone(),
        manifest: loaded.manifest.clone(),
        attempt: loaded.attempts.get(&task).cloned(),
    }
}

/// `_release_unverified`: every child that is not verified leaves the stack, its task back to
/// `ready`, with a comment that says why.
fn release_unverified(
    loaded: &LandWorld,
    stamp: &Stamp,
    start: Run,
    stack: &Stack,
    parent_key: &str,
) -> Result<(Run, Vec<TaskKey>), WgError> {
    let items: Vec<LandItem> = item_rows(&start, stack.id)
        .iter()
        .filter_map(|item| {
            start.applied.tasks.task(item.task_id).map(|task| LandItem {
                key: task.key.to_string(),
                number: task.number,
                merge_order: item.merge_order,
                state: item.state,
                task_status: task.status,
                detail: item.detail.clone(),
            })
        })
        .collect();
    let stack_id = loaded.stack_id.to_string();
    unverified_children(&items).into_iter().try_fold(
        (start, Vec::new()),
        |(run, released), item| {
            let task = run
                .applied
                .tasks
                .tasks
                .values()
                .find(|task| task.key.to_string() == item.key)
                .cloned()
                .ok_or_else(|| missing(&item.key))?;
            let plan = wg_stacks::landing::release_unverified(
                parent_key,
                &item.key,
                item.state,
                item.task_status,
                &stack_id,
            );
            let moved = match &plan.ready_reason {
                Some(reason) => {
                    let next = transition(
                        &run.applied.tasks,
                        &run.stacks,
                        stamp,
                        run.applied.ids,
                        task.id,
                        TaskStatus::Ready,
                        Change {
                            reason: Some(reason),
                            ..Change::default()
                        },
                    )?;
                    let applied = wg_tasks::transition::Applied {
                        decision: run.applied.decision.clone().then(next.decision),
                        ids: next.ids,
                        tasks: next.tasks,
                    };
                    Run {
                        stacks: run.stacks.apply_all(
                            applied
                                .decision
                                .writes
                                .iter()
                                .skip(run.applied.decision.writes.len()),
                        ),
                        applied,
                    }
                }
                None => run.clone(),
            };
            let data = plan.comment_data.as_object().cloned().unwrap_or_default();
            let comment = comment_step(
                stamp,
                moved.applied.ids,
                task.id,
                task.workspace_id,
                CommentKind::Comment,
                &plan.comment_body,
                &data,
                None,
                None,
            );
            let commented = Run {
                applied: wg_tasks::transition::Applied {
                    decision: moved.applied.decision.clone().then(comment.decision),
                    ids: comment.ids,
                    tasks: moved.applied.tasks.apply(&Write::insert(comment.row)),
                },
                stacks: moved.stacks.clone(),
            };
            let (done, _) = release_child(&commented, stamp, task.id)?;
            Ok::<_, WgError>((
                done,
                released.into_iter().chain([task.key.clone()]).collect(),
            ))
        },
    )
}

/// `_finish`: the unverified children are released, the verified children and the parent are
/// accepted, the stack becomes `landed` and its request is cleared. The reply is the parent's
/// acceptance.
pub fn finish_landing(
    loaded: &LandWorld,
    stamp: &Stamp,
    tips: &[(String, String)],
    mode: &str,
    reason: &str,
) -> Result<Decision<Value>, WgError> {
    let stack = loaded
        .world
        .stacks
        .stack(loaded.stack_id)
        .ok_or_else(|| missing("stack"))?
        .clone();
    let parent = loaded
        .world
        .tasks
        .task(stack.parent_task_id)
        .ok_or_else(|| missing("the stack's parent task"))?
        .clone();
    let parent_key = parent.key.to_string();
    let start = Run::start(&loaded.world, stamp);
    let (released_run, released) = release_unverified(loaded, stamp, start, &stack, &parent_key)?;
    let verified: Vec<Id> = item_rows(&released_run, stack.id)
        .iter()
        .filter(|item| item.state == StackItemState::Verified)
        .map(|item| item.task_id)
        .collect();
    let (children_run, accepted) =
        verified
            .iter()
            .try_fold((released_run, Vec::new()), |(run, keys), task| {
                let (next, _) = accept_in(
                    &run,
                    stamp,
                    *task,
                    "automated",
                    &child_accept_reason(&parent_key),
                    &facts_of(loaded, *task),
                )?;
                let key = next
                    .applied
                    .tasks
                    .task(*task)
                    .map(|found| found.key.clone())
                    .ok_or_else(|| missing("task"))?;
                Ok::<_, WgError>((next, keys.into_iter().chain([key]).collect::<Vec<_>>()))
            })?;
    let (landed_run, acceptance) = accept_in(
        &children_run,
        stamp,
        parent.id,
        mode,
        reason,
        &facts_of(loaded, parent.id),
    )?;
    let (status_event, ids) = landed_run.applied.ids.next();
    let (landed_event, _) = ids.next();
    let landed_stack = Stack {
        status: StackStatus::Landed,
        land_request: None,
        updated_at: stamp.at,
        ..stack.clone()
    };
    let released_keys: VectorSync<TaskKey> = released.into_iter().collect();
    let tail = Decision::new(())
        .write(Write::update(landed_stack))
        .event(EventDraft::new(
            stamp,
            status_event,
            Some(stack.workspace_id),
            stack.id,
            EventPayload::StackStatusChanged(StackStatusChanged {
                from: stack.status,
                to: StackStatus::Landed,
                reason: landed_reason(&parent_key),
            }),
        ))
        .event(EventDraft::new(
            stamp,
            landed_event,
            Some(stack.workspace_id),
            stack.id,
            EventPayload::StackLanded(StackLanded {
                parent_key: parent.key.clone(),
                mode: mode.to_owned(),
                reason: reason.to_owned(),
                run_id: loaded.run_id,
                tips: StrMap(tips.iter().cloned().collect()),
                tasks: accepted.into_iter().collect(),
                released: released_keys,
            }),
        ));
    Ok(landed_run
        .applied
        .decision
        .then(tail)
        .with_reply(acceptance_from_model(&acceptance)))
}

/// `worktrees.remove` after git removed the directory: the row is marked removed and the
/// `worktree.removed` event recorded.
pub fn worktree_removed(stamp: &Stamp, row: &ManagedWorktree) -> Decision<()> {
    let (id, _) = stamp.ids().next();
    Decision::new(())
        .write(Write::update(ManagedWorktree {
            status: WorktreeStatus::Removed,
            removed_at: Some(stamp.at),
            updated_at: stamp.at,
            ..row.clone()
        }))
        .event(
            EventDraft::new(
                stamp,
                id,
                Some(row.workspace_id),
                row.id,
                EventPayload::WorktreeRemoved(WorktreeRemoved {
                    path: row.path.clone(),
                    branch_kept: row.branch.clone(),
                }),
            )
            .with_attempt(row.attempt_id),
        )
}

/// `_remove_integration`: the `stack.integration_removed` event of one repository.
pub fn integration_removed(
    stamp: &Stamp,
    stack: &Stack,
    repository: &str,
    branch: &str,
) -> Decision<()> {
    let (id, _) = stamp.ids().next();
    Decision::new(()).event(EventDraft::new(
        stamp,
        id,
        Some(stack.workspace_id),
        stack.id,
        EventPayload::StackIntegrationRemoved(StackIntegrationRemoved {
            repository: repository.to_owned(),
            branch: branch.to_owned(),
        }),
    ))
}

/// `_integration_busy` over stored jobs: integration work for this stack is queued or running.
pub fn integration_busy_of(jobs: &[wg_model::entity::Job], stack: Id) -> bool {
    let refs: Vec<JobRef> = jobs
        .iter()
        .map(|job| JobRef {
            kind: job.kind.clone(),
            status: job.status.to_string(),
            stack_id: job
                .payload
                .get("stack_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
        })
        .collect();
    integration_busy(&refs, &stack.to_string())
}

/// What a `stack.verified` event recorded: the run id and the integration tips.
pub fn verified_from(
    data: &serde_json::Map<String, serde_json::Value>,
) -> (Option<Id>, BTreeMap<String, String>) {
    let run = data
        .get("run_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|text| wg_types::parse_id(text).ok());
    let tips = data
        .get("tips")
        .and_then(serde_json::Value::as_object)
        .map(|found| {
            found
                .iter()
                .filter_map(|(name, tip)| tip.as_str().map(|t| (name.clone(), t.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    (run, tips)
}

/// The checkout (or the source repository) a fast-forward move runs in.
pub fn move_dir(step: &FfMove, source: &str) -> String {
    match &step.step {
        FfStep::MergeFfOnly { holder } => holder.clone(),
        FfStep::UpdateRef => source.to_owned(),
    }
}

/// A path as `Path` spells it: git prints forward slashes, Windows paths use backslashes.
pub fn native_path(path: &str) -> String {
    if cfg!(windows) {
        path.replace('/', "\\")
    } else {
        path.to_owned()
    }
}

/// `directory_name`: the repository name as a directory name.
pub fn directory_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}
