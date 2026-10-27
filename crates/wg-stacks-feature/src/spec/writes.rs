//! The stack writes: `StackService.create`, `add_item`, `remove_item` and `readmit_item`, the
//! item transition they share, and the jobs `integrate` and `verify` queue
//! (`StackService.integrate`, `StackVerifier.request`), as decisions over the task world.
//!
//! Each function checks the permission, resolves the references the request names, applies the
//! pure rules of `wg-stacks` and returns a [`Decision`] whose reply is the output value the
//! command prints. What only git can say (a new stack's base commits, the tip of an attempt
//! branch, the tip of an integration worktree) comes in as a value the shell read.

use serde_json::json;
use wg_jobs::limits::stack_key;
use wg_jobs::queue::{NewJob, enqueue};
use wg_json::{Value, dict};
use wg_model::entity::{Repository, RepositoryCheckout, Stack, StackItem, StackRepository, Task};
use wg_model::event::{
    EventDraft, EventPayload, StackCreated, StackItemAdded, StackItemRemoved,
    StackItemStateChanged, StrMap,
};
use wg_model::slice::{QueueSlice, StackSlice};
use wg_model::stamp::{IdSeq, Stamp};
use wg_model::{Decision, TaskSlice, Write, py_repr_str};
use wg_registry::checkout::primary_checkout;
use wg_stacks::item::{check_item_transition, merge_order_after, satisfies_stack_dependency};
use wg_stacks::membership::{
    check_add_item, check_readmit, check_remove_item, outside_repositories, readmit_task_path,
};
use wg_stacks::verify::{verify_job_key, verify_resource_keys};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::transition::{Applied, Change, refresh_dependents, transition};
use wg_types::{Id, Permission, StackItemState, StackStatus, TaskStatus, WgError};
use wg_wire::convention::{id as id_value, text};

use super::out::{Tips, stack_out};
use super::reads::{TipTarget, tip_directory};
use super::{active_stack, item_of, task_in};
use wg_tasks::accept::substamp;

// ---------------------------------------------------------------- create

/// What `create` found in the world: the parent, its children and the repositories they link.
#[derive(Debug, Clone)]
pub struct CreatePlan<'a> {
    pub parent: &'a Task,
    /// The parent's unarchived children, by task number.
    pub children: Vec<&'a Task>,
    /// The repositories linked by the parent or a child, by name.
    pub repositories: Vec<&'a Repository>,
}

/// The checks `StackService.create` makes before it reads git.
pub fn create_plan<'a>(
    world: &'a TaskWorld,
    scope: Id,
    parent_ref: &str,
) -> Result<CreatePlan<'a>, WgError> {
    let parent = task_in(world, scope, parent_ref)?;
    let mut children: Vec<&Task> = world
        .tasks
        .tasks
        .values()
        .filter(|task| task.parent_id == Some(parent.id) && task.archived_at.is_none())
        .collect();
    children.sort_by_key(|task| task.number);
    if children.is_empty() {
        return Err(
            WgError::validation(format!("{} has no child tasks to stack", parent.key))
                .detail("parent", parent.key.to_string()),
        );
    }
    if world.stacks.active_stack_of_parent(parent.id).is_some() {
        return Err(
            WgError::conflict(format!("{} already has an active stack", parent.key))
                .detail("parent", parent.key.to_string()),
        );
    }
    let linked: std::collections::BTreeSet<Id> = world
        .tasks
        .task_repositories
        .keys()
        .filter(|(task_id, _)| {
            *task_id == parent.id || children.iter().any(|child| child.id == *task_id)
        })
        .map(|(_, repository_id)| *repository_id)
        .collect();
    let mut repositories: Vec<&Repository> = linked
        .iter()
        .filter_map(|repository_id| world.tasks.registry.repository(*repository_id))
        .collect();
    repositories.sort_by(|a, b| a.name.cmp(&b.name));
    if repositories.is_empty() {
        return Err(WgError::validation(format!(
            "{} and its children are not linked to any repository",
            parent.key
        ))
        .detail("parent", parent.key.to_string()));
    }
    Ok(CreatePlan {
        parent,
        children,
        repositories,
    })
}

/// `repository <name> has no available checkout`.
#[must_use]
pub fn no_checkout(repository: &str) -> WgError {
    WgError::validation(format!(
        "repository {} has no available checkout",
        py_repr_str(repository)
    ))
}

/// `StackService._base_sha`, before git: the checkout directory and the default branch whose head
/// is the base of a new stack in `repository`.
pub fn base_source(
    repository: &Repository,
    checkouts: &[RepositoryCheckout],
) -> Result<(String, String), WgError> {
    let branch = repository
        .default_branch
        .clone()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            WgError::validation(format!(
                "repository {} has no default branch",
                py_repr_str(&repository.name)
            ))
        })?;
    let checkout =
        primary_checkout(checkouts, true).ok_or_else(|| no_checkout(&repository.name))?;
    Ok((checkout.path.clone(), branch))
}

/// `StackService.create`: a stack of the parent's children. `base_shas` pairs each repository name
/// with the head of its default branch, in repository order.
pub fn create(
    world: &TaskWorld,
    stamp: &Stamp,
    scope: Id,
    parent_ref: &str,
    base_shas: &[(String, String)],
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::StackWrite])?;
    let plan = create_plan(world, scope, parent_ref)?;
    let branch = format!("wg/stack/{}", plan.parent.key.to_string().to_lowercase());
    let sha_of = |name: &str| {
        base_shas
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, sha)| sha.clone())
            .ok_or_else(|| WgError::workgraph(format!("no base commit for repository {name}")))
    };
    let (stack_id, ids) = stamp.ids().next();
    let stack = Stack {
        id: stack_id,
        workspace_id: scope,
        parent_task_id: plan.parent.id,
        status: StackStatus::Open,
        created_at: stamp.at,
        updated_at: stamp.at,
        land_request: None,
    };
    let repositories = plan
        .repositories
        .iter()
        .map(|repository| {
            sha_of(&repository.name).map(|base_sha| StackRepository {
                stack_id,
                repository_id: repository.id,
                base_sha,
                integration_branch: branch.clone(),
                created_at: stamp.at,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (items, ids) = plan
        .children
        .iter()
        .fold((Vec::new(), ids), |(items, ids), child| {
            let (item_id, ids) = ids.next();
            let item = StackItem {
                id: item_id,
                stack_id,
                task_id: child.id,
                state: StackItemState::Pending,
                merge_order: None,
                detail: None,
                created_at: stamp.at,
                updated_at: stamp.at,
            };
            (items.into_iter().chain([item]).collect(), ids)
        });
    let (event_id, _) = ids.next();
    let payload = StackCreated {
        parent_key: plan.parent.key.clone(),
        integration_branch: branch.clone(),
        base_shas: StrMap(
            plan.repositories
                .iter()
                .filter_map(|repository| {
                    sha_of(&repository.name)
                        .ok()
                        .map(|sha| (repository.name.clone(), sha))
                })
                .collect(),
        ),
        tasks: plan
            .children
            .iter()
            .map(|child| child.key.clone())
            .collect(),
    };
    let decision = std::iter::once(Write::insert(stack.clone()))
        .chain(repositories.into_iter().map(Write::insert))
        .chain(items.into_iter().map(Write::insert))
        .fold(Decision::new(()), Decision::write)
        .event(EventDraft::new(
            stamp,
            event_id,
            Some(scope),
            stack_id,
            EventPayload::StackCreated(payload),
        ));
    let after = TaskWorld {
        tasks: world.tasks.clone(),
        stacks: world.stacks.apply_all(decision.writes.iter()),
    };
    let reply = stack_out(&after, &stack, &Tips::new())?;
    Ok(decision.with_reply(reply))
}

// ---------------------------------------------------------------- add and remove

/// `StackService.add_item`: a child of the parent joins its active stack.
pub fn add_item(
    world: &TaskWorld,
    stamp: &Stamp,
    scope: Id,
    parent_ref: &str,
    task_ref: &str,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::StackWrite])?;
    let parent = task_in(world, scope, parent_ref)?;
    let stack = active_stack(world, parent)?;
    let child = task_in(world, scope, task_ref)?;
    let child_repositories: Vec<Id> = world
        .tasks
        .task_repositories
        .keys()
        .filter(|(task_id, _)| *task_id == child.id)
        .map(|(_, repository_id)| *repository_id)
        .collect();
    let stack_repositories: Vec<Id> = world
        .stacks
        .repositories_of(stack.id)
        .map(|row| row.repository_id)
        .collect();
    let names: Vec<(Id, String)> = world
        .tasks
        .registry
        .repositories
        .values()
        .map(|repository| (repository.id, repository.name.clone()))
        .collect();
    check_add_item(
        &child.key.to_string(),
        &parent.key.to_string(),
        child.parent_id == Some(parent.id),
        world
            .stacks
            .items_of(stack.id)
            .any(|item| item.task_id == child.id),
        &outside_repositories(&child_repositories, &stack_repositories, &names),
    )?;
    let (item_id, ids) = stamp.ids().next();
    let (event_id, _) = ids.next();
    let item = StackItem {
        id: item_id,
        stack_id: stack.id,
        task_id: child.id,
        state: StackItemState::Pending,
        merge_order: None,
        detail: None,
        created_at: stamp.at,
        updated_at: stamp.at,
    };
    let decision = Decision::new(())
        .write(Write::insert(item))
        .write(Write::update(Stack {
            updated_at: stamp.at,
            ..stack.clone()
        }))
        .event(EventDraft::new(
            stamp,
            event_id,
            Some(stack.workspace_id),
            stack.id,
            EventPayload::StackItemAdded(StackItemAdded {
                task_key: child.key.clone(),
                parent_key: parent.key.clone(),
            }),
        ));
    let after = TaskWorld {
        tasks: world.tasks.clone(),
        stacks: world.stacks.apply_all(decision.writes.iter()),
    };
    let reply = stack_out(&after, stack, &Tips::new())?;
    Ok(decision.with_reply(reply))
}

/// `StackService.remove_item`: a child that is not on the integration branch leaves the stack.
pub fn remove_item(
    world: &TaskWorld,
    stamp: &Stamp,
    scope: Id,
    parent_ref: &str,
    task_ref: &str,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::StackWrite])?;
    let parent = task_in(world, scope, parent_ref)?;
    let stack = active_stack(world, parent)?;
    let child = task_in(world, scope, task_ref)?;
    let item = item_of(world, stack, child)?;
    let state = item.state;
    check_remove_item(&child.key.to_string(), state)?;
    let (event_id, ids) = stamp.ids().next();
    let base = Decision::new(())
        .write(Write::delete(item.row_key()))
        .write(Write::update(Stack {
            updated_at: stamp.at,
            ..stack.clone()
        }))
        .event(EventDraft::new(
            stamp,
            event_id,
            Some(stack.workspace_id),
            stack.id,
            EventPayload::StackItemRemoved(StackItemRemoved {
                task_key: child.key.clone(),
                parent_key: parent.key.clone(),
                state,
            }),
        ));
    let stacks = world.stacks.apply_all(base.writes.iter());
    let start = Applied {
        decision: base,
        ids,
        tasks: world.tasks.clone(),
    };
    // Dependents that relied on this sibling inside the stack must wait for acceptance again.
    let applied = if satisfies_stack_dependency(state) {
        refresh_dependents(start, &stacks, stamp, child.id, event_id)?
    } else {
        start
    };
    let after = TaskWorld {
        tasks: applied.tasks,
        stacks,
    };
    let reply = stack_out(
        &after,
        after.stacks.stack(stack.id).unwrap_or(stack),
        &Tips::new(),
    )?;
    Ok(applied.decision.with_reply(reply))
}

// ---------------------------------------------------------------- the item transition

/// What a change leaves behind for the next one: the changes so far, the slices after them and the
/// ids still free.
#[derive(Clone)]
pub(crate) struct Step {
    pub(crate) decision: Decision<()>,
    pub(crate) tasks: TaskSlice,
    pub(crate) stacks: StackSlice,
    pub(crate) ids: IdSeq,
}

/// The `stack_integrate` job of each repository of `stack` (in repository-id order), keyed by the
/// stack state `version` so that repeated requests for an unchanged stack collapse into one job
/// (`StackService.enqueue_integration`). `queue` holds the jobs already stored under those keys.
pub(crate) fn integration_jobs(
    stack: &Stack,
    repository_ids: &[Id],
    version: Id,
    stamp: &Stamp,
    queue: &QueueSlice,
) -> Vec<(Id, Decision<Id>)> {
    repository_ids
        .iter()
        .enumerate()
        .map(|(index, repository_id)| {
            let mut job = NewJob::new("stack_integrate");
            job.payload = json!({
                "stack_id": stack.id.to_string(),
                "repository_id": repository_id.to_string(),
            })
            .as_object()
            .cloned()
            .unwrap_or_default();
            job.workspace_id = Some(stack.workspace_id);
            job.idempotency_key = Some(integration_key(stack.id, *repository_id, version));
            job.resource_keys = vec![stack_key(&stack.id.to_string(), &repository_id.to_string())];
            job.causation_id = Some(version);
            job.created_by = stamp.actor.to_string();
            let salt = 100 + u32::try_from(index).unwrap_or(0);
            (*repository_id, enqueue(queue, &job, &substamp(stamp, salt)))
        })
        .collect()
}

/// The idempotency key of an integration job.
pub(crate) fn integration_key(stack_id: Id, repository_id: Id, version: Id) -> String {
    format!("stack-integrate:{stack_id}:{repository_id}:{version}")
}

/// The repository ids of a stack, in id order.
pub(crate) fn repository_ids(stacks: &StackSlice, stack_id: Id) -> Vec<Id> {
    let mut ids: Vec<Id> = stacks
        .repositories_of(stack_id)
        .map(|row| row.repository_id)
        .collect();
    ids.sort();
    ids
}

/// `StackService.transition_item`: moves an item to `target`, records the change, refreshes the
/// dependents whose sibling dependency it flips, and queues integration when the item is ready.
#[allow(clippy::too_many_arguments)]
pub(crate) fn transition_item(
    start: &Step,
    stamp: &Stamp,
    stack: &Stack,
    parent: &Task,
    child: &Task,
    item: &StackItem,
    target: StackItemState,
    detail: Option<String>,
) -> Result<Step, WgError> {
    let current = item.state;
    if current == target {
        return Ok(Step {
            decision: Decision::new(()),
            tasks: start.tasks.clone(),
            stacks: start.stacks.clone(),
            ids: start.ids,
        });
    }
    check_item_transition(current, target)?;
    let stack_max = start
        .stacks
        .items_of(stack.id)
        .filter_map(|row| row.merge_order)
        .max();
    let moved = StackItem {
        state: target,
        detail: detail.clone(),
        merge_order: merge_order_after(item.merge_order, target, stack_max),
        updated_at: stamp.at,
        ..item.clone()
    };
    let (event_id, ids) = start.ids.next();
    let base = Decision::new(())
        .write(Write::update(moved.clone()))
        .write(Write::update(Stack {
            updated_at: stamp.at,
            ..stack.clone()
        }))
        .event(EventDraft::new(
            stamp,
            event_id,
            Some(stack.workspace_id),
            stack.id,
            EventPayload::StackItemStateChanged(StackItemStateChanged {
                task_key: child.key.clone(),
                parent_key: parent.key.clone(),
                from: current,
                to: target,
                detail,
                merge_order: moved.merge_order,
            }),
        ));
    let stacks = start.stacks.apply_all(base.writes.iter());
    let applied = Applied {
        decision: base,
        ids,
        tasks: start.tasks.clone(),
    };
    // Entering or leaving integration-ready changes whether dependents on this sibling are satisfied.
    let applied = if satisfies_stack_dependency(current) != satisfies_stack_dependency(target) {
        refresh_dependents(applied, &stacks, stamp, child.id, event_id)?
    } else {
        applied
    };
    let decision = if target == StackItemState::Ready {
        integration_jobs(
            stack,
            &repository_ids(&stacks, stack.id),
            event_id,
            stamp,
            &QueueSlice::default(),
        )
        .into_iter()
        .flat_map(|(_, job)| job.writes.iter().cloned().collect::<Vec<_>>())
        .fold(applied.decision, Decision::write)
    } else {
        applied.decision
    };
    Ok(Step {
        decision,
        tasks: applied.tasks,
        stacks,
        ids: applied.ids,
    })
}

// ---------------------------------------------------------------- readmit

/// A repository of a stack whose recorded head a readmit compares with the attempt branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadmitProbe {
    pub repository_id: Id,
    pub name: String,
}

/// The checks of `StackService.readmit_item` that come before git: the readmitted task's id and
/// key, and the repositories whose recorded head the shell then looks up.
pub fn readmit_probes(
    world: &TaskWorld,
    scope: Id,
    parent_ref: &str,
    task_ref: &str,
) -> Result<(Id, String, Vec<ReadmitProbe>), WgError> {
    let parent = task_in(world, scope, parent_ref)?;
    let stack = active_stack(world, parent)?;
    let child = task_in(world, scope, task_ref)?;
    let item = item_of(world, stack, child)?;
    check_readmit(&child.key.to_string(), item.state)?;
    let mut probes: Vec<ReadmitProbe> = world
        .stacks
        .repositories_of(stack.id)
        .filter_map(|row| world.tasks.registry.repository(row.repository_id))
        .map(|repository| ReadmitProbe {
            repository_id: repository.id,
            name: repository.name.clone(),
        })
        .collect();
    probes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((child.id, child.key.to_string(), probes))
}

/// An attempt branch of a stack child: the head recorded at submit and the branch tip now
/// (`None` when the branch no longer exists).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedBranch {
    pub repository: String,
    pub branch: String,
    pub head: String,
    pub tip: Option<String>,
}

/// The tip an unchecked `rev-parse --verify --quiet refs/heads/<branch>` printed; `None` for a
/// branch that no longer exists.
#[must_use]
pub fn branch_tip(stdout: &str) -> Option<String> {
    Some(stdout.trim().to_owned()).filter(|tip| !tip.is_empty())
}

/// `StackService._refuse_unrecorded_commits`: a readmit whose attempt branch moved after submit
/// would leave those commits out of the integration.
pub fn check_recorded(task_key: &str, recorded: &[RecordedBranch]) -> Result<(), WgError> {
    match recorded
        .iter()
        .find(|entry| entry.tip.as_deref().is_some_and(|tip| tip != entry.head))
    {
        None => Ok(()),
        Some(entry) => {
            let tip = entry.tip.clone().unwrap_or_default();
            Err(WgError::conflict(format!(
                    "{task_key}'s attempt branch {} in {} is at {tip}, but the head recorded \
                     at submit is {}: commits added after submit would be left out of the readmit. \
                     Resubmit through a new attempt (attempt start, then submit) so the new head is recorded.",
                    entry.branch,
                    py_repr_str(&entry.repository),
                    entry.head
                ))
            .detail("task", task_key)
            .detail("repository", entry.repository.clone())
            .detail("branch", entry.branch.clone())
            .detail("branch_tip", tip)
            .detail("recorded_head", entry.head.clone()))
        }
    }
}

/// `StackService.readmit_item`: an ejected (or re-added, pending) child is queued for integration
/// again without a new fast gate.
pub fn readmit(
    world: &TaskWorld,
    stamp: &Stamp,
    scope: Id,
    parent_ref: &str,
    task_ref: &str,
    recorded: &[RecordedBranch],
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::StackWrite])?;
    let parent = task_in(world, scope, parent_ref)?;
    let stack = active_stack(world, parent)?;
    let child = task_in(world, scope, task_ref)?;
    let item = item_of(world, stack, child)?;
    check_readmit(&child.key.to_string(), item.state)?;
    check_recorded(&child.key.to_string(), recorded)?;
    let start = Step {
        decision: Decision::new(()),
        tasks: world.tasks.clone(),
        stacks: world.stacks.clone(),
        ids: stamp.ids(),
    };
    let moved = transition_item(
        &start,
        stamp,
        stack,
        parent,
        child,
        item,
        StackItemState::Ready,
        item.detail.clone(),
    )?;
    // An ejection returned the task to ready; its submitted change comes back with the item, so
    // the task goes back to awaiting verification (through running, as the lifecycle requires),
    // or the stack could verify and still refuse to land it.
    let reason = format!("readmitted to stack {}", parent.key);
    let status = moved
        .tasks
        .task(child.id)
        .map_or(child.status, |task| task.status);
    let finished = readmit_task_path(status).iter().try_fold(
        Applied {
            decision: moved.decision,
            ids: moved.ids,
            tasks: moved.tasks,
        },
        |so_far, target: &TaskStatus| -> Result<Applied, WgError> {
            let step = transition(
                &so_far.tasks,
                &moved.stacks,
                stamp,
                so_far.ids,
                child.id,
                *target,
                Change {
                    reason: Some(&reason),
                    ..Change::default()
                },
            )?;
            Ok(Applied {
                decision: so_far.decision.then(step.decision),
                ids: step.ids,
                tasks: step.tasks,
            })
        },
    )?;
    let after = TaskWorld {
        tasks: finished.tasks,
        stacks: moved.stacks,
    };
    let reply = stack_out(
        &after,
        after.stacks.stack(stack.id).unwrap_or(stack),
        &Tips::new(),
    )?;
    Ok(finished.decision.with_reply(reply))
}

// ---------------------------------------------------------------- integrate and verify

/// The stack an integration or verification request is about, and its repositories.
#[derive(Debug, Clone)]
pub struct Queued {
    pub stack: Stack,
    pub parent_key: String,
    /// The repositories of the stack, by name, with the base commit of each.
    pub repositories: Vec<(Id, String, String)>,
}

/// The parent's active stack with its repositories (the reads of `integrate` and `request`).
pub fn queued(world: &TaskWorld, scope: Id, parent_ref: &str) -> Result<Queued, WgError> {
    let parent = task_in(world, scope, parent_ref)?;
    let stack = active_stack(world, parent)?;
    let mut repositories: Vec<(Id, String, String)> = world
        .stacks
        .repositories_of(stack.id)
        .filter_map(|row| {
            world
                .tasks
                .registry
                .repository(row.repository_id)
                .map(|repository| (repository.id, repository.name.clone(), row.base_sha.clone()))
        })
        .collect();
    repositories.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(Queued {
        stack: stack.clone(),
        parent_key: parent.key.to_string(),
        repositories,
    })
}

/// The idempotency keys `integrate` enqueues under for a stack state `version`.
pub fn integration_keys(queued: &Queued, version: Id) -> Vec<String> {
    queued
        .repositories
        .iter()
        .map(|(repository, _, _)| integration_key(queued.stack.id, *repository, version))
        .collect()
}

/// `StackService.integrate`: one `stack_integrate` job per repository of the active stack.
pub fn integrate(
    stamp: &Stamp,
    queued: &Queued,
    version: Id,
    queue: &QueueSlice,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::StackWrite])?;
    let mut ids: Vec<Id> = queued.repositories.iter().map(|(id, _, _)| *id).collect();
    ids.sort();
    let jobs = integration_jobs(&queued.stack, &ids, version, stamp, queue);
    let listed = jobs
        .iter()
        .map(|(repository, job)| {
            let name = queued
                .repositories
                .iter()
                .find(|(id, _, _)| id == repository)
                .map(|(_, name, _)| name.as_str())
                .unwrap_or_default();
            dict!({
                "repository": text(name),
                "job_id": id_value(job.reply),
            })
        })
        .collect();
    let decision = jobs
        .iter()
        .flat_map(|(_, job)| job.writes.iter().cloned().collect::<Vec<_>>())
        .fold(Decision::new(()), Decision::write);
    Ok(decision.with_reply(dict!({
        "stack": text(&queued.parent_key),
        "jobs": Value::List(listed),
    })))
}

/// The integration worktrees whose tips a verification request keys its job by.
#[must_use]
pub fn verification_targets(queued: &Queued) -> Vec<TipTarget> {
    queued
        .repositories
        .iter()
        .map(|(_, name, _)| TipTarget {
            stack_id: queued.stack.id,
            repository: name.clone(),
            directory: tip_directory(queued.stack.id, name),
        })
        .collect()
}

/// The integration tip of each repository, in name order; its base while the worktree does not
/// exist yet (`StackVerifier._tips`).
#[must_use]
pub fn tips_or_bases(queued: &Queued, found: &Tips) -> Vec<(String, String)> {
    queued
        .repositories
        .iter()
        .map(|(_, name, base)| {
            (
                name.clone(),
                found
                    .get(&(queued.stack.id, name.clone()))
                    .filter(|tip| !tip.is_empty())
                    .cloned()
                    .unwrap_or_else(|| base.clone()),
            )
        })
        .collect()
}

/// The idempotency key of the `stack_verify` job for the integration `tips` (repository name and
/// tip, in name order) and the stack state `version`.
pub fn verification_key(queued: &Queued, tips: &[(String, String)], version: Id) -> String {
    verify_job_key(
        &queued.stack.id.to_string(),
        tips,
        Some(&version.to_string()),
    )
}

/// `StackVerifier.request`: one `stack_verify` job for the active stack's integration tips.
pub fn request_verification(
    stamp: &Stamp,
    queued: &Queued,
    tips: &[(String, String)],
    version: Id,
    queue: &QueueSlice,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::StackWrite])?;
    let stack_id = queued.stack.id.to_string();
    let mut job = NewJob::new("stack_verify");
    job.payload = json!({"stack_id": stack_id})
        .as_object()
        .cloned()
        .unwrap_or_default();
    job.workspace_id = Some(queued.stack.workspace_id);
    job.idempotency_key = Some(verification_key(queued, tips, version));
    job.resource_keys = verify_resource_keys(
        &stack_id,
        &queued
            .repositories
            .iter()
            .map(|(id, _, _)| id.to_string())
            .collect::<Vec<_>>(),
    );
    job.causation_id = Some(version);
    job.created_by = stamp.actor.to_string();
    let queued_job = enqueue(queue, &job, &substamp(stamp, 200));
    let reply = dict!({
        "stack": text(&queued.parent_key),
        "job_id": id_value(queued_job.reply),
    });
    Ok(queued_job.with_reply(reply))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wg_types::ErrorCode;

    fn branch(tip: Option<&str>) -> RecordedBranch {
        RecordedBranch {
            repository: "svc".to_owned(),
            branch: "wg/attempt".to_owned(),
            head: "aaa".to_owned(),
            tip: tip.map(str::to_owned),
        }
    }

    #[test]
    fn a_branch_that_moved_after_submit_refuses_the_readmit() {
        let error = check_recorded("WG-2", &[branch(Some("bbb"))]).err();
        assert_eq!(error.as_ref().map(|e| e.code), Some(ErrorCode::Conflict));
        assert!(
            error
                .map(|e| e.message)
                .is_some_and(|m| m.contains("is at bbb, but the head recorded at submit is aaa"))
        );
    }

    #[test]
    fn an_unmoved_or_deleted_branch_passes() {
        assert!(check_recorded("WG-2", &[branch(Some("aaa")), branch(None)]).is_ok());
    }

    #[test]
    fn a_branch_tip_is_the_trimmed_output() {
        assert_eq!(
            branch_tip(
                "abc
"
            ),
            Some("abc".to_owned())
        );
        assert_eq!(
            branch_tip(
                "
"
            ),
            None
        );
    }
}
