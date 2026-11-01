//! `StackLander`: the transactions of landing a verified stack, rebasing it onto a default branch
//! that moved, and releasing a child from it.
//!
//! The checks that come before anything moves are `wg_stacks::landing`'s. Git (the default
//! branches, the integration tips, the fast-forwards) and the order of the steps are the
//! shell's.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use wg_jobs::queue::NewJob;
use wg_model::entity::{RevisionManifest, Stack, StackRepository, TaskAcceptance, VerificationRun};
use wg_model::event::{
    EventPayload, StackIntegrationRemoved, StackItemReleased, StackLandRequested, StackLanded,
    StackLandingRefused, StackRebased, TaskAccepted,
};
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::{Decision, Write};
use wg_rows::task_world_sql::TaskWorld;
use wg_stacks::landing::blockers_first;
use wg_stacks::landing::{
    ACCEPTABLE_TASK_STATUSES, LandDecision, LandItem, LandRepo, LandState, Refusal,
    UnverifiedRelease, child_accept_reason, land_decision, landed_reason, release_plan,
    release_unverified, verified_run,
};
use wg_stacks::landing::{Dependent, ReleasePlan};
use wg_stacks::rebuild::{StackTask, dependents};
use wg_tasks::deps::unaccepted_dependencies;
use wg_tasks::transition::Change;
use wg_types::{ErrorCode, Id, StackItemState, StackStatus, TaskStatus, WgError, parse_id};

use super::chain::Chain;
use super::text::dumps_sorted;
use super::{item_rows, str_map};

/// The stored landing request (`json.loads(stack.land_request)`): its mode and reason.
#[must_use]
pub fn parse_land_request(text: &str) -> Option<wg_stacks::landing::LandRequest> {
    let value: Value = serde_json::from_str(text).ok()?;
    Some(wg_stacks::landing::LandRequest {
        mode: value.get("mode")?.as_str()?.to_owned(),
        reason: value.get("reason")?.as_str()?.to_owned(),
    })
}

/// Whether a failure is one `land` records as a refused landing and `after_verified` reports as
/// `refused` (Python's `ConflictError` and `ValidationFailed`, with their subclasses).
#[must_use]
pub fn is_refusal(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::Conflict
            | ErrorCode::VersionConflict
            | ErrorCode::InvalidTransition
            | ErrorCode::ResourceBusy
            | ErrorCode::ValidationFailed
            | ErrorCode::DependencyCycle
    )
}

// ---------------------------------------------------------------- what landing reads

/// One repository of the stack as landing reads it from the rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRow {
    pub id: Id,
    pub name: String,
    pub default_branch: Option<String>,
    pub base_sha: String,
    pub integration_branch: String,
}

/// Everything `_land` reads from the rows before it asks git.
#[derive(Debug, Clone, PartialEq)]
pub struct LandBase {
    pub parent_key: String,
    pub stack_id: Id,
    pub stack_status: StackStatus,
    pub parent_status: TaskStatus,
    pub items: Vec<LandItem>,
    /// The stack's repositories, by repository name.
    pub repos: Vec<RepoRow>,
}

/// The rows `_land` reads.
pub fn land_base(world: &TaskWorld, stack_id: Id) -> Result<LandBase, WgError> {
    let stack = world
        .stacks
        .stack(stack_id)
        .ok_or_else(|| WgError::not_found(format!("stack {stack_id} not found")))?;
    let parent = world.tasks.task(stack.parent_task_id).ok_or_else(|| {
        WgError::workgraph(format!(
            "task {} vanished from the slice",
            stack.parent_task_id
        ))
    })?;
    let items = item_rows(&world.stacks, &world.tasks, stack_id)
        .into_iter()
        .map(|(item, task)| LandItem {
            key: task.key.to_string(),
            number: task.number,
            merge_order: item.merge_order,
            state: item.state,
            task_status: task.status,
            detail: item.detail.clone(),
        })
        .collect();
    let mut repos: Vec<RepoRow> = world
        .stacks
        .repositories_of(stack_id)
        .filter_map(|link| {
            world
                .tasks
                .registry
                .repository(link.repository_id)
                .map(|repository| RepoRow {
                    id: repository.id,
                    name: repository.name.clone(),
                    default_branch: repository.default_branch.clone(),
                    base_sha: link.base_sha.clone(),
                    integration_branch: link.integration_branch.clone(),
                })
        })
        .collect();
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(LandBase {
        parent_key: parent.key.to_string(),
        stack_id,
        stack_status: stack.status,
        parent_status: parent.status,
        items,
        repos,
    })
}

/// The preconditions that need no git: everything `_land` checks before it resolves a branch.
pub fn precheck(base: &LandBase, verified: Option<&Value>) -> Result<(), WgError> {
    let state = state_of(base, verified, &[]);
    land_decision(&state)
        .map(|_| ())
        .map_err(|refusal| refusal.to_error())
}

/// What git said about one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFacts {
    pub repository: String,
    /// Where the integration branch is now.
    pub integrated_tip: String,
    /// Where the default branch is now.
    pub default_head: String,
}

fn state_of(base: &LandBase, verified: Option<&Value>, facts: &[GitFacts]) -> LandState {
    let tips = verified_run(&base.stack_id.to_string(), verified)
        .map(|(_, tips)| tips)
        .unwrap_or_default();
    LandState {
        parent_key: base.parent_key.clone(),
        stack_id: base.stack_id.to_string(),
        stack_status: base.stack_status,
        parent_status: base.parent_status,
        items: base.items.clone(),
        verified_event: verified.is_some(),
        repos: facts
            .iter()
            .filter_map(|found| {
                base.repos
                    .iter()
                    .find(|repo| repo.name == found.repository)
                    .map(|repo| LandRepo {
                        id: repo.id.to_string(),
                        name: repo.name.clone(),
                        default_branch: repo.default_branch.clone(),
                        base_sha: repo.base_sha.clone(),
                        integrated_tip: found.integrated_tip.clone(),
                        default_head: found.default_head.clone(),
                    })
            })
            .collect(),
        tips,
    }
}

/// `_land` after git has been asked: land, or rebase and keep the request.
pub fn decide(
    base: &LandBase,
    verified: Option<&Value>,
    facts: &[GitFacts],
) -> Result<LandDecision, WgError> {
    land_decision(&state_of(base, verified, facts)).map_err(|refusal: Refusal| refusal.to_error())
}

/// `_verified_run`: the passing run and the tips the stack last verified.
pub fn verified(
    stack_id: Id,
    event: Option<&Value>,
) -> Result<(Option<Id>, BTreeMap<String, String>), WgError> {
    let (run, tips) =
        verified_run(&stack_id.to_string(), event).map_err(|refusal| refusal.to_error())?;
    Ok((run.map(|text| parse_id(&text)).transpose()?, tips))
}

// ---------------------------------------------------------------- refusals and bases

/// `land`'s record of a refused landing.
pub fn landing_refused(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    mode: &str,
    reason: &str,
    message: &str,
) -> Result<Decision<()>, WgError> {
    let chain = Chain::start(world, stamp);
    let workspace = chain.stack(stack_id)?.workspace_id;
    Ok(chain
        .event(
            workspace,
            stack_id,
            EventPayload::StackLandingRefused(StackLandingRefused {
                mode: mode.to_owned(),
                reason: reason.to_owned(),
                error: message.to_owned(),
            }),
        )
        .finish(()))
}

/// Why the bases move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moved {
    /// A default branch moved before the stack verified (`rebase_if_stale`).
    Rebased,
    /// A default branch moved after: the landing request is kept (`_defer`).
    Deferred { mode: String, reason: String },
}

/// `_move_bases`: each repository's base points at its new head, and the move is recorded.
pub fn move_bases(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    heads: &BTreeMap<String, String>,
    moved: &Moved,
) -> Result<Decision<()>, WgError> {
    let chain = Chain::start(world, stamp);
    let stack = chain.stack(stack_id)?.clone();
    let parent_key = chain.task(stack.parent_task_id)?.key.clone();
    let links: Vec<(StackRepository, String)> = chain
        .repositories(stack_id)
        .into_iter()
        .filter_map(|link| {
            world
                .tasks
                .registry
                .repository(link.repository_id)
                .map(|repository| (link.clone(), repository.name.clone()))
        })
        .collect();
    let previous: BTreeMap<String, String> = links
        .iter()
        .map(|(link, name)| (name.clone(), link.base_sha.clone()))
        .collect();
    let chain = links.iter().try_fold(chain, |chain, (link, name)| {
        let head = heads
            .get(name)
            .cloned()
            .ok_or_else(|| WgError::workgraph(format!("no new base for repository {name}")))?;
        Ok::<_, WgError>(chain.write(Write::update(StackRepository {
            base_sha: head,
            ..link.clone()
        })))
    })?;
    let actor = stamp.actor.to_string();
    let request = match moved {
        Moved::Rebased => None,
        Moved::Deferred { mode, reason } => Some(dumps_sorted(&json!({
            "mode": mode,
            "reason": reason,
            "actor": actor,
        }))),
    };
    let chain = chain.write(Write::update(Stack {
        land_request: request.or(stack.land_request.clone()),
        updated_at: stamp.at,
        ..stack.clone()
    }));
    let payload = match moved {
        Moved::Rebased => EventPayload::StackRebased(StackRebased {
            parent_key,
            reason: wg_stacks::landing::REBASED_REASON.to_owned(),
            base_shas: str_map(previous.iter()),
            new_base_shas: str_map(heads.iter()),
        }),
        Moved::Deferred { mode, reason } => EventPayload::StackLandRequested(StackLandRequested {
            parent_key,
            mode: mode.clone(),
            reason: reason.clone(),
            base_shas: str_map(previous.iter()),
            new_base_shas: str_map(heads.iter()),
        }),
    };
    Ok(chain
        .event(stack.workspace_id, stack_id, payload)
        .finish(()))
}

// ---------------------------------------------------------------- acceptance

/// What accepting a task reads that the rows of the stack do not hold.
#[derive(Debug, Clone, Default)]
pub struct AcceptFacts {
    /// The run the acceptance links, and its manifest.
    pub run: Option<VerificationRun>,
    pub manifest: Option<RevisionManifest>,
    /// The latest succeeded attempt of each task.
    pub attempts: BTreeMap<Id, Id>,
}

/// `VerificationService._accept`: the acceptance row, its event, the task's move to `accepted`
/// and the change-surface job.
pub fn accept_task<'a>(
    chain: Chain<'a>,
    task_id: Id,
    mode: &str,
    reason: &str,
    facts: &AcceptFacts,
) -> Result<Chain<'a>, WgError> {
    let task = chain.task(task_id)?.clone();
    if !matches!(
        task.status,
        TaskStatus::AwaitingReview | TaskStatus::AwaitingVerification
    ) {
        return Err(WgError::invalid_transition(format!(
            "{} is {}; only completed work awaiting verification or review can be accepted",
            task.key,
            task.status.as_str()
        )));
    }
    let attempt = facts.attempts.get(&task_id).copied();
    let stamp = chain.stamp;
    let run_id = facts.run.as_ref().map(|run| run.id);
    let (manifest_id, chain) = match (&facts.run, &facts.manifest) {
        (Some(run), Some(source)) if run.manifest_id.is_some() => {
            let (id, chain) = chain.next_id();
            let accepted = RevisionManifest {
                id,
                workspace_id: task.workspace_id,
                attempt_id: attempt,
                kind: "accepted".to_owned(),
                entries: source.entries.clone(),
                created_at: stamp.at,
            };
            (Some(id), chain.write(Write::insert(accepted)))
        }
        _ => (None, chain),
    };
    let (acceptance_id, chain) = chain.next_id();
    let chain = chain.write(Write::insert(TaskAcceptance {
        id: acceptance_id,
        task_id,
        attempt_id: attempt,
        verification_run_id: run_id,
        decision: "accepted".to_owned(),
        mode: mode.to_owned(),
        reason: reason.to_owned(),
        actor: stamp.actor.to_string(),
        manifest_id,
        created_at: stamp.at,
    }));
    let (event_id, chain) = chain.event_for_attempt(
        task.workspace_id,
        task_id,
        EventPayload::TaskAccepted(TaskAccepted {
            mode: mode.to_owned(),
            reason: reason.to_owned(),
            verification_run_id: run_id,
            manifest_id,
        }),
        None,
        attempt,
    );
    let moved = format!("{mode} acceptance");
    let chain = chain.task_to(
        task_id,
        TaskStatus::Accepted,
        Change {
            reason: Some(&moved),
            privileged: true,
            caused_by: Some(event_id),
            ..Change::default()
        },
    )?;
    let mut job = NewJob::new("compute_change_surface");
    job.payload = [("task_id".to_owned(), Value::String(task_id.to_string()))]
        .into_iter()
        .collect();
    job.workspace_id = Some(task.workspace_id);
    job.task_id = Some(task_id);
    job.idempotency_key = Some(format!("surface:{task_id}:{acceptance_id}"));
    job.causation_id = Some(event_id);
    job.correlation_id = Some(stamp.correlation.clone());
    Ok(chain.enqueue(&job, &QueueSlice::default()))
}

// ---------------------------------------------------------------- release

/// `release_child`: a child leaves its active stack. Returns whether its commits were on the
/// integration branch (so the caller rebuilds it).
pub fn release_child<'a>(
    chain: Chain<'a>,
    task_id: Id,
) -> Result<(Chain<'a>, Option<Id>), WgError> {
    let found = chain
        .step
        .stacks
        .stacks
        .values()
        .filter(|stack| !wg_stacks::merge::is_inactive(stack.status))
        .find_map(|stack| {
            chain
                .step
                .stacks
                .items_of(stack.id)
                .find(|item| item.task_id == task_id)
                .map(|item| (stack.clone(), item.clone()))
        });
    let Some((stack, item)) = found else {
        return Ok((chain, None));
    };
    let parent = chain.task(stack.parent_task_id)?.clone();
    let task = chain.task(task_id)?.clone();
    let all_tasks: Vec<StackTask> = item_rows(&chain.step.stacks, &chain.step.tasks, stack.id)
        .iter()
        .map(|(_, row)| StackTask {
            task_id: row.id.to_string(),
            number: row.number,
        })
        .collect();
    let edges: Vec<(String, String)> = chain
        .step
        .tasks
        .dependencies
        .values()
        .map(|edge| (edge.task_id.to_string(), edge.depends_on_id.to_string()))
        .collect();
    let ordered = dependents(&all_tasks, &edges, &task_id.to_string());
    let states: Vec<Dependent> = ordered
        .iter()
        .filter_map(|id| parse_id(id).ok())
        .filter_map(|dependent| {
            chain
                .step
                .stacks
                .items_of(stack.id)
                .find(|row| row.task_id == dependent)
                .map(|row| Dependent {
                    item_id: row.id.to_string(),
                    task_key: chain
                        .step
                        .tasks
                        .task(dependent)
                        .map(|row| row.key.to_string())
                        .unwrap_or_default(),
                    state: row.state,
                })
        })
        .collect();
    let plan: ReleasePlan = release_plan(
        &task.key.to_string(),
        &parent.key.to_string(),
        item.state,
        &states,
    );
    let chain = plan.reset.iter().try_fold(chain, |chain, reset| {
        let item_id = parse_id(&reset.item_id)?;
        let dependent_task = chain.item(item_id)?.task_id;
        chain.item_to(
            stack.id,
            dependent_task,
            StackItemState::Pending,
            Some(dumps_sorted(&reset.detail)),
        )
    })?;
    let chain = chain
        .event(
            stack.workspace_id,
            stack.id,
            EventPayload::StackItemReleased(StackItemReleased {
                task_key: task.key.clone(),
                parent_key: parent.key.clone(),
                state: item.state,
                rebuild: plan.rebuild,
            }),
        )
        .write(Write::delete(item.row_key()))
        .touch_stack(stack.id);
    Ok((chain, plan.rebuild.then_some(stack.id)))
}

/// `_release_unverified`: every unverified child goes back to ready and leaves the stack.
fn release_unverified_children<'a>(
    chain: Chain<'a>,
    stack_id: Id,
    parent_key: &str,
) -> Result<(Chain<'a>, Vec<String>), WgError> {
    let rows = item_rows(&chain.step.stacks, &chain.step.tasks, stack_id);
    rows.iter()
        .filter(|(item, _)| item.state != StackItemState::Verified)
        .try_fold((chain, Vec::new()), |(chain, released), (item, task)| {
            let current = chain.task(task.id)?.status;
            let plan: UnverifiedRelease = release_unverified(
                parent_key,
                &task.key.to_string(),
                item.state,
                current,
                &stack_id.to_string(),
            );
            let chain = match &plan.ready_reason {
                Some(why) => chain.task_to(
                    task.id,
                    TaskStatus::Ready,
                    Change {
                        reason: Some(why),
                        ..Change::default()
                    },
                )?,
                None => chain,
            };
            let data: Map<String, Value> =
                plan.comment_data.as_object().cloned().unwrap_or_default();
            let chain = chain.comment(task.id, &plan.comment_body, &data, None)?;
            let (chain, _) = release_child(chain, task.id)?;
            Ok::<_, WgError>((
                chain,
                released
                    .into_iter()
                    .chain(std::iter::once(task.key.to_string()))
                    .collect(),
            ))
        })
}

/// `_finish`: the acceptances are recorded and the stack is marked landed. The reply is the
/// parent's acceptance id.
pub fn finish_landing(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    tips: &BTreeMap<String, String>,
    mode: &str,
    reason: &str,
    facts: &AcceptFacts,
) -> Result<Decision<Id>, WgError> {
    let chain = Chain::start(world, stamp);
    let stack = chain.stack(stack_id)?.clone();
    let parent = chain.task(stack.parent_task_id)?.clone();
    let parent_key = parent.key.to_string();
    let (chain, released) = release_unverified_children(chain, stack_id, &parent_key)?;
    // Blockers first, so a dependent whose blockers are accepted in this pass is accepted too. A
    // dependent is skipped only when one of its blockers stays unaccepted.
    let rows = item_rows(&chain.step.stacks, &chain.step.tasks, stack_id);
    let in_stack: BTreeSet<Id> = rows.iter().map(|(_, task)| task.id).collect();
    let mut blockers: BTreeMap<Id, BTreeSet<Id>> = BTreeMap::new();
    for edge in chain.step.tasks.dependencies.values() {
        if in_stack.contains(&edge.task_id) && in_stack.contains(&edge.depends_on_id) {
            blockers
                .entry(edge.task_id)
                .or_default()
                .insert(edge.depends_on_id);
        }
    }
    let verified: Vec<_> = blockers_first(rows, |(_, task)| task.id, &blockers)
        .into_iter()
        .filter(|(item, _)| item.state == StackItemState::Verified)
        .map(|(_, task)| task)
        .collect();
    let child_reason = child_accept_reason(&parent_key);
    let (chain, accepted_keys) =
        verified
            .iter()
            .try_fold((chain, Vec::new()), |(chain, mut accepted), task| {
                if !unaccepted_dependencies(&chain.step.tasks, task.id).is_empty() {
                    return Ok::<_, WgError>((chain, accepted));
                }
                let chain = accept_task(chain, task.id, "automated", &child_reason, facts)?;
                accepted.push(task.key.clone());
                Ok((chain, accepted))
            })?;
    let chain = accept_task(chain, parent.id, mode, reason, facts)?;
    let acceptance = chain
        .step
        .decision
        .writes
        .iter()
        .rev()
        .find_map(|write| match write {
            Write::Insert(wg_model::Row::TaskAcceptance(row)) if row.task_id == parent.id => {
                Some(row.id)
            }
            _ => None,
        })
        .ok_or_else(|| {
            WgError::workgraph(format!(
                "acceptance of {} vanished from the slice",
                parent.id
            ))
        })?;
    let released_keys: Vec<wg_types::TaskKey> = released
        .iter()
        .filter_map(|key| wg_types::TaskKey::parse(key).ok())
        .collect();
    let chain = chain.stack_status(stack_id, StackStatus::Landed, &landed_reason(&parent_key));
    let current = chain.stack(stack_id)?.clone();
    let chain = chain
        .write(Write::update(Stack {
            land_request: None,
            updated_at: stamp.at,
            ..current
        }))
        .event(
            stack.workspace_id,
            stack_id,
            EventPayload::StackLanded(StackLanded {
                parent_key: parent.key.clone(),
                mode: mode.to_owned(),
                reason: reason.to_owned(),
                run_id: facts.run.as_ref().map(|run| run.id),
                tips: str_map(tips.iter()),
                tasks: accepted_keys.into_iter().collect(),
                released: released_keys.into_iter().collect(),
            }),
        );
    Ok(chain.finish(acceptance))
}

/// `_remove_integration`: an integration branch was removed after landing.
pub fn integration_removed(
    world: &TaskWorld,
    stamp: &Stamp,
    stack_id: Id,
    repository: &str,
    branch: &str,
) -> Result<Decision<()>, WgError> {
    let chain = Chain::start(world, stamp);
    let workspace = chain.stack(stack_id)?.workspace_id;
    Ok(chain
        .event(
            workspace,
            stack_id,
            EventPayload::StackIntegrationRemoved(StackIntegrationRemoved {
                repository: repository.to_owned(),
                branch: branch.to_owned(),
            }),
        )
        .finish(()))
}

/// Task statuses `land` accepts a child or the parent in.
#[must_use]
pub fn acceptable(status: TaskStatus) -> bool {
    ACCEPTABLE_TASK_STATUSES.contains(&status)
}
