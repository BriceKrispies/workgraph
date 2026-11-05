//! The attempt writes of the `attempt` command group (`AttemptService.create` and `finish`, and
//! `RunService.start_external`, `abandon_external` and the failure finalisation): each reads what
//! its Python session read, asks [`crate::spec::attempt_writes`] and the `wg-exec` plans what to
//! write, and applies the answer as one decision per Python session.

use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Map, json};
use wg_exec::attempt::{
    ACTIVE_STATUSES, VERIFIER_PID_KEY, abandon_plan, adopt_check, is_terminal, killed_adoption,
    next_attempt_number, start_plan,
};
use wg_exec::run::EarlyEnd;
use wg_git::Git;
use wg_json::Value;
use wg_model::Decision;
use wg_model::Write;
use wg_model::entity::{AgentAttempt, ChangeSet, ChangeSetItem, Repository};
use wg_model::stamp::py_repr_str;
use wg_model::write::RowKey;
use wg_proc::process_alive;
use wg_registry::checkout::primary_checkout;
use wg_rows::task_world_sql::TaskWorld;
use wg_runtime::spec::RequestContext;
use wg_runtime::spec::effects::GIT_TIMEOUT_MS;
use wg_runtime::{App, index_written};
use wg_store::Session;
use wg_tasks::accept::active_item;
use wg_tasks::transition::Applied;
use wg_types::{
    AttemptStatus, ChangeSetItemStatus, ChangeSetStatus, Id, Permission, TaskStatus, WgError,
};
use wg_wire::convention::json_value;
use wg_wire::dto::attempt;

use super::conclude::{gate_adopted, load_submit};
use super::resolve::{find_attempt, find_task};
use super::worktrees::{worktree_prepare, worktree_release};
use crate::spec::attempt_writes::{
    Finish, NewAttempt, attempt_limit, claimed, claimed_chain, created, escalated, failures,
    finished, gone, reason_text, start_chain, start_out, status_set, task_moved,
};
use crate::spec::conclude::fail_verification;
use crate::spec::worktrees::PrepareRequest;

/// `decide` for decisions that may write tasks: the search index follows the writes in the same
/// transaction, as it does for every task service.
pub(super) trait DecideWritten {
    fn decide_written<S, R>(
        &mut self,
        context: &RequestContext,
        load: impl FnOnce(&mut Session<'_>) -> Result<S, WgError>,
        decide: impl FnOnce(&S, &wg_model::Stamp) -> Result<Decision<R>, WgError>,
    ) -> Result<wg_runtime::Outcome<R>, WgError>;
}

impl DecideWritten for wg_runtime::Runtime {
    fn decide_written<S, R>(
        &mut self,
        context: &RequestContext,
        load: impl FnOnce(&mut Session<'_>) -> Result<S, WgError>,
        decide: impl FnOnce(&S, &wg_model::Stamp) -> Result<Decision<R>, WgError>,
    ) -> Result<wg_runtime::Outcome<R>, WgError> {
        self.decide_then(context, load, decide, index_written)
    }
}

/// The rows a decision about one attempt reads: the attempt, its task's world and the task's
/// attempts (for the next number and the failure count).
pub(super) struct AttemptWorld {
    pub(super) attempt: AgentAttempt,
    pub(super) world: TaskWorld,
    pub(super) attempts: Vec<AgentAttempt>,
}

pub(super) fn load_attempt_world(
    session: &mut Session<'_>,
    attempt_id: Id,
) -> Result<AttemptWorld, WgError> {
    let attempt = session
        .attempt_by_id(attempt_id)?
        .ok_or_else(|| WgError::not_found(format!("attempt {attempt_id} not found")))?;
    let world = session.task_world(Some(attempt.workspace_id), &[])?;
    let attempts = session.attempts_of_task(attempt.task_id, &[])?;
    Ok(AttemptWorld {
        attempt,
        world,
        attempts,
    })
}

/// What `attempt start` is asked for.
pub struct StartRequest<'a> {
    pub workspace: Option<&'a str>,
    pub task: &'a str,
    pub agent_name: Option<&'a str>,
    pub adapter: &'a str,
    pub model: Option<&'a str>,
    pub isolation: Option<&'a str>,
}

/// `RunService.start_external`: claims the task for the actor (or continues the actor's claim),
/// records an attempt, prepares its worktrees and moves task and attempt to running. The reply is
/// `AttemptStartOut`.
pub fn attempt_start(
    app: &mut App,
    context: &RequestContext,
    request: &StartRequest<'_>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskClaim)?;
    app.require_permission(context, Permission::AttemptWrite)?;
    if let Some(isolation) = request
        .isolation
        .filter(|text| *text != "worktree" && *text != "in_place")
    {
        return Err(WgError::validation(format!(
            "isolation must be worktree or in_place, not {}",
            py_repr_str(isolation)
        )));
    }
    let actor = context.actor.to_string();
    let agent = request
        .agent_name
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| match actor.split_once(':') {
            Some((_, name)) if !name.is_empty() => name.to_owned(),
            _ => actor.clone(),
        });
    let scope = app.workspace_or_all(request.workspace)?;
    let (task, active) = app.runtime.store.read(|session| {
        let task = find_task(session, scope, request.task)?;
        let active = session.attempts_of_task(task.id, ACTIVE_STATUSES)?;
        Ok((task, active))
    })?;
    let active_number = active
        .iter()
        .find(|row| row.parent_attempt_id.is_none())
        .map(|row| row.number);
    let plan = start_plan(
        &actor,
        &task.key.to_string(),
        task.status,
        task.claimed_by.as_deref() == Some(actor.as_str()),
        task.claimed_by.as_deref(),
        active_number,
    )?;
    let spec = NewAttempt {
        provider: agent.clone(),
        adapter: request.adapter.to_owned(),
        status: AttemptStatus::Queued,
        agent_id: Some(actor.clone()),
        model: request.model.map(str::to_owned),
        session_id: None,
        parent_attempt_id: None,
        config_snapshot: [
            ("agent", json!(agent)),
            ("model", json!(request.model)),
            ("external", json!(true)),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect(),
    };
    let (attempt_id, number) = app
        .runtime
        .decide_then(
            context,
            |session| {
                let world = session.task_world(Some(task.workspace_id), &[])?;
                let numbers: Vec<i64> = session
                    .attempts_of_task(task.id, &[])?
                    .iter()
                    .map(|row| row.number)
                    .collect();
                Ok((world, numbers))
            },
            |(world, numbers), stamp| {
                // One session, as Python's: the claim, the attempt and its first move.
                let begun = start_chain(stamp, &world.tasks);
                let chain = if plan.claim_first {
                    claimed_chain(world, stamp, task.workspace_id, task.id)?
                } else {
                    begun
                };
                let current = chain
                    .tasks
                    .task(task.id)
                    .ok_or_else(|| gone(task.id))?
                    .clone();
                let (chain, row) = created(
                    chain,
                    stamp,
                    &current,
                    next_attempt_number(numbers),
                    spec.clone(),
                );
                let (chain, row) = status_set(chain, stamp, &row, AttemptStatus::Preparing)?;
                Ok(chain.decision.with_reply((row.id, row.number)))
            },
            index_written,
        )?
        .reply;
    let manifest = match worktree_prepare(app, context, attempt_id, None, request.isolation, &[]) {
        Ok(manifest) => manifest,
        Err(err) => {
            let end = EarlyEnd::external_start_failed(&err.message);
            let _ = finalize_failure(app, context, attempt_id, &end);
            return Err(err);
        }
    };
    if let Err(err) = run_attempt(app, context, attempt_id, number) {
        let end = EarlyEnd::external_start_failed(&err.message);
        let _ = finalize_failure(app, context, attempt_id, &end);
        return Err(err);
    }
    Ok(json_value(&start_out(attempt_id, number, &task, &manifest)))
}

/// The second session of `start_external`: a claimed task and the attempt move to running.
fn run_attempt(
    app: &mut App,
    context: &RequestContext,
    attempt_id: Id,
    number: i64,
) -> Result<(), WgError> {
    app.runtime.decide_written(
        context,
        |session| load_attempt_world(session, attempt_id),
        |loaded, stamp| {
            let task = loaded
                .world
                .tasks
                .task(loaded.attempt.task_id)
                .ok_or_else(|| gone(loaded.attempt.task_id))?;
            let chain = start_chain(stamp, &loaded.world.tasks);
            let chain = if task.status == TaskStatus::Claimed {
                task_moved(
                    chain,
                    &loaded.world.stacks,
                    stamp,
                    task.id,
                    TaskStatus::Running,
                    &format!("attempt #{number}"),
                )?
            } else {
                chain
            };
            let (chain, _) = status_set(chain, stamp, &loaded.attempt, AttemptStatus::Running)?;
            Ok(chain.decision)
        },
    )?;
    Ok(())
}

/// `RunService._finalize_failure`: an attempt that ends without a conclusion is finished, the task
/// it holds is failed (or returned to ready when cancelled) and escalated, and the attempt's
/// worktrees are released.
pub fn finalize_failure(
    app: &mut App,
    context: &RequestContext,
    attempt_id: Id,
    end: &EarlyEnd,
) -> Result<(), WgError> {
    let limit = attempt_limit(&app.config()?);
    app.runtime.decide_written(
        context,
        |session| load_attempt_world(session, attempt_id),
        |loaded, stamp| {
            let chain = start_chain(stamp, &loaded.world.tasks);
            let (chain, row) = if is_terminal(loaded.attempt.status) {
                (chain, loaded.attempt.clone())
            } else {
                finished(
                    chain,
                    stamp,
                    &loaded.attempt,
                    end.status,
                    Finish {
                        exit_reason: Some(end.exit_reason.clone()),
                        error: Some(end.error.clone()),
                        ..Finish::default()
                    },
                )?
            };
            let task_id = row.task_id;
            let chain = match loaded
                .world
                .tasks
                .task(task_id)
                .and_then(|task| end.task_after(task.status))
            {
                Some(target) => {
                    let moved = task_moved(
                        chain,
                        &loaded.world.stacks,
                        stamp,
                        task_id,
                        target,
                        &end.reason(),
                    )?;
                    escalated(
                        moved,
                        stamp,
                        task_id,
                        failures(&loaded.attempts, &row),
                        limit,
                    )
                }
                None => chain,
            };
            Ok(chain.decision)
        },
    )?;
    worktree_release(app, context, attempt_id, false, false)?;
    Ok(())
}

/// `RunService.abandon_external`: an external attempt whose agent stopped without submitting is
/// abandoned with the reason, its task returns to ready and its worktrees are released.
pub fn attempt_abandon(
    app: &mut App,
    context: &RequestContext,
    reference: &str,
    reason: &str,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskClaim)?;
    app.require_permission(context, Permission::AttemptWrite)?;
    if reason.trim().is_empty() {
        return Err(WgError::validation("abandoning an attempt needs a reason"));
    }
    let found = app
        .runtime
        .store
        .read(|session| find_attempt(session, reference))?;
    let (task_key, attempt_id) = app
        .runtime
        .decide_written(
            context,
            |session| load_attempt_world(session, found.id),
            |loaded, stamp| {
                let attempt = &loaded.attempt;
                let task = loaded
                    .world
                    .tasks
                    .task(attempt.task_id)
                    .ok_or_else(|| gone(attempt.task_id))?;
                let external = attempt
                    .config_snapshot
                    .get("external")
                    .is_some_and(|flag| flag.as_bool().unwrap_or(false));
                let target = abandon_plan(
                    external,
                    &attempt.id.to_string(),
                    attempt.status,
                    reason,
                    task.status,
                )?;
                let (chain, _) = finished(
                    start_chain(stamp, &loaded.world.tasks),
                    stamp,
                    attempt,
                    AttemptStatus::Abandoned,
                    Finish {
                        exit_reason: Some("abandoned".to_owned()),
                        error: Some(reason.to_owned()),
                        ..Finish::default()
                    },
                )?;
                let chain = match target {
                    Some(target) => task_moved(
                        chain,
                        &loaded.world.stacks,
                        stamp,
                        task.id,
                        target,
                        &reason_text(&format!("attempt #{} abandoned: {reason}", attempt.number)),
                    )?,
                    None => chain,
                };
                Ok(chain
                    .decision
                    .with_reply((task.key.to_string(), attempt.id)))
            },
        )?
        .reply;
    worktree_release(app, context, attempt_id, false, false)?;
    Ok(json_value(&json!({
        "attempt_id": attempt_id.to_string(),
        "task": task_key,
        "attempt_status": "abandoned",
        "task_status": "ready",
    })))
}

/// What `attempt create` records (`AttemptCreate`).
pub struct CreateRequest<'a> {
    pub workspace: Option<&'a str>,
    pub task: &'a str,
    pub provider: &'a str,
    pub adapter: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub model: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub parent_attempt: Option<&'a str>,
    pub status: AttemptStatus,
}

/// `AttemptService.create`: an attempt by an externally running agent. A running attempt of a
/// claimed task moves the task to running.
pub fn attempt_create(
    app: &mut App,
    context: &RequestContext,
    request: &CreateRequest<'_>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::AttemptWrite)?;
    let scope = app.workspace_or_all(request.workspace)?;
    let (task, parent) = app.runtime.store.read(|session| {
        let task = find_task(session, scope, request.task)?;
        let parent = match request.parent_attempt.filter(|text| !text.is_empty()) {
            Some(reference) => {
                let parent = find_attempt(session, reference)?;
                if parent.workspace_id != task.workspace_id {
                    return Err(WgError::validation(
                        "parent attempt belongs to another workspace",
                    ));
                }
                Some(parent.id)
            }
            None => None,
        };
        Ok((task, parent))
    })?;
    let actor = context.actor.to_string();
    let spec = NewAttempt {
        provider: request.provider.to_owned(),
        adapter: request
            .adapter
            .filter(|text| !text.is_empty())
            .unwrap_or(request.provider)
            .to_owned(),
        status: request.status,
        agent_id: Some(
            request
                .agent_id
                .filter(|text| !text.is_empty())
                .map_or_else(|| actor.clone(), str::to_owned),
        ),
        model: request.model.map(str::to_owned),
        session_id: request.session_id.map(str::to_owned),
        parent_attempt_id: parent,
        config_snapshot: Map::new(),
    };
    let row = app
        .runtime
        .decide_written(
            context,
            |session| {
                let world = session.task_world(Some(task.workspace_id), &[])?;
                let numbers: Vec<i64> = session
                    .attempts_of_task(task.id, &[])?
                    .iter()
                    .map(|row| row.number)
                    .collect();
                Ok((world, numbers))
            },
            |(world, numbers), stamp| {
                let current = world.tasks.task(task.id).ok_or_else(|| gone(task.id))?;
                let (chain, row) = created(
                    start_chain(stamp, &world.tasks),
                    stamp,
                    current,
                    next_attempt_number(numbers),
                    spec.clone(),
                );
                let holds = request.status == AttemptStatus::Running
                    && current.status == TaskStatus::Claimed;
                let chain = if holds {
                    let moved = task_moved(
                        chain,
                        &world.stacks,
                        stamp,
                        task.id,
                        TaskStatus::Running,
                        &format!("attempt #{} started", row.number),
                    )?;
                    claimed(moved, task.id, &actor)
                } else {
                    chain
                };
                Ok(chain.decision.with_reply(row))
            },
        )?
        .reply;
    Ok(attempt::from_model(&row))
}

/// `AttemptService.finish`: an externally-run attempt reported as finished.
pub fn attempt_finish(
    app: &mut App,
    context: &RequestContext,
    reference: &str,
    status: &str,
    summary: &str,
    error: Option<&str>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::AttemptWrite)?;
    let target = AttemptStatus::parse(status)
        .ok_or_else(|| WgError::validation(format!("{status} is not a valid attempt status")))?;
    let found = app
        .runtime
        .store
        .read(|session| find_attempt(session, reference))?;
    let row = app
        .runtime
        .decide(
            context,
            |session| load_attempt_world(session, found.id),
            |loaded, stamp| {
                let (chain, row) = finished(
                    start_chain(stamp, &loaded.world.tasks),
                    stamp,
                    &loaded.attempt,
                    target,
                    Finish {
                        exit_reason: Some("reported".to_owned()),
                        summary: Some(summary.to_owned()),
                        error: error.map(str::to_owned),
                        ..Finish::default()
                    },
                )?;
                Ok(chain.decision.with_reply(row))
            },
        )?
        .reply;
    Ok(attempt::from_model(&row))
}

/// The failure an adopted attempt gets when the process that verified it was killed (WG-4285).
const KILLED_VERIFIER: &str = "the process verifying this adopted ref was killed before the verification finished; adopt the ref again to verify it";

/// WG-4285: a killed `attempt adopt` leaves its attempt in `verifying`, which holds the task for
/// good. The task's adopted attempts whose verifying process is gone fail first, so adopting the
/// ref again can run.
fn fail_killed_adoptions(
    app: &mut App,
    context: &RequestContext,
    scope: Option<Id>,
    task_ref: &str,
) -> Result<(), WgError> {
    let stuck: Vec<Id> = app.runtime.store.read(|session| {
        let task = find_task(session, scope, task_ref)?;
        Ok(session
            .attempts_of_task(task.id, &[AttemptStatus::Verifying])?
            .into_iter()
            .filter(|row| killed_adoption(row.status, &row.config_snapshot, process_alive))
            .map(|row| row.id)
            .collect())
    })?;
    for attempt_id in stuck {
        app.runtime.decide_then(
            context,
            |session| {
                session
                    .attempt_by_id(attempt_id)?
                    .ok_or_else(|| WgError::not_found(format!("attempt {attempt_id} not found")))
            },
            |attempt, stamp| fail_verification(attempt, KILLED_VERIFIER, stamp),
            index_written,
        )?;
    }
    Ok(())
}

/// `AttemptService.adopt` (WG-4067): records a succeeded attempt whose head in one repository is
/// `reference`, so work made outside Workgraph can enter a stack without a manual merge. The
/// attempt's baseline is the merge-base of `reference` with the stack base (or the repository's
/// default branch outside a stack). The task ends `awaiting_verification`, and a stack child is
/// run through the same gate as a submit (WG-4072): the attempt gets a worktree at the ref, and a
/// stack child's fast checks run there. A pass marks the stack item ready and the task stays
/// awaiting verification; a failure fails the attempt and the task, as a submit does. A task with
/// no fast checks is marked ready without a run, as a submit marks it.
pub fn attempt_adopt(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_ref: &str,
    reference: &str,
    repository: Option<&str>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskClaim)?;
    app.require_permission(context, Permission::AttemptWrite)?;
    app.require_permission(context, Permission::VerifyRun)?;
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(WgError::validation("adopt needs a ref to adopt"));
    }
    let scope = app.workspace_or_all(workspace)?;
    fail_killed_adoptions(app, context, scope, task_ref)?;
    let actor = context.actor.to_string();
    let (task, world, repo, checkouts, has_active, covering) =
        app.runtime.store.read(|session| {
            let task = find_task(session, scope, task_ref)?;
            let world = session.task_world(Some(task.workspace_id), &[])?;
            let repo = match repository.filter(|name| !name.is_empty()) {
                Some(name) => session
                    .repositories(task.workspace_id, None, false)?
                    .into_iter()
                    .find(|row| row.name == name)
                    .ok_or_else(|| WgError::not_found(format!("repository {name} not found")))?,
                None => {
                    let linked: Vec<Id> = world
                        .tasks
                        .repositories_of(task.id)
                        .map(|link| link.repository_id)
                        .collect();
                    let only = match linked.as_slice() {
                        [only] => *only,
                        [] => {
                            return Err(WgError::validation(
                                "task has no linked repository; pass --repository NAME",
                            ));
                        }
                        _ => {
                            return Err(WgError::validation(
                                "task links several repositories; pass --repository NAME",
                            ));
                        }
                    };
                    session
                        .fetch::<Repository>(&RowKey::Repository(only))?
                        .ok_or_else(|| WgError::not_found(format!("repository {only} not found")))?
                }
            };
            let checkouts = session.checkouts(repo.id)?;
            let has_active = !session
                .attempts_of_task(task.id, ACTIVE_STATUSES)?
                .is_empty();
            let covering = session.active_repository_for_task(task.id, repo.id)?;
            Ok((task, world, repo, checkouts, has_active, covering))
        })?;
    adopt_check(&task.key.to_string(), task.status, has_active)?;
    let stack_state = active_item(&world.stacks, task.id).map(|(_, item)| item.state);
    if stack_state.is_some() {
        app.require_permission(context, Permission::StackWrite)?;
    }
    let checkout = primary_checkout(&checkouts, true).ok_or_else(|| {
        WgError::validation(format!(
            "repository '{}' has no available checkout",
            repo.name
        ))
    })?;
    let git = Git {
        path: PathBuf::from(&checkout.path),
        timeout_ms: GIT_TIMEOUT_MS,
    };
    let git_error = |error: wg_git::GitError| WgError::git(error.to_string());
    let head = git.resolve_commit(reference).map_err(|_| {
        WgError::validation(format!(
            "ref {reference} does not resolve to a commit in repository '{}'",
            repo.name
        ))
    })?;
    let base_ref = match &covering {
        Some(row) => row.base_sha.clone(),
        None => match repo.default_branch.clone() {
            Some(branch) => branch,
            None => git
                .default_branch()
                .map_err(git_error)?
                .ok_or_else(|| WgError::validation("repository has no default branch"))?,
        },
    };
    let base = git.resolve_commit(&base_ref).map_err(git_error)?;
    let baseline = git
        .merge_base(&base, &head)
        .map_err(git_error)?
        .ok_or_else(|| {
            WgError::validation(format!("{reference} shares no history with {base_ref}"))
        })?;
    let commits = git.rev_list_range(&baseline, &head).map_err(git_error)?;

    let task_id = task.id;
    let (attempt_id, number) = app
        .runtime
        .decide_written(
            context,
            |session| {
                let world = session.task_world(Some(task.workspace_id), &[])?;
                let numbers: Vec<i64> = session
                    .attempts_of_task(task.id, &[])?
                    .iter()
                    .map(|row| row.number)
                    .collect();
                let active = !session
                    .attempts_of_task(task.id, ACTIVE_STATUSES)?
                    .is_empty();
                Ok((world, numbers, active))
            },
            |(world, numbers, active), stamp| {
                let current = world.tasks.task(task_id).ok_or_else(|| gone(task_id))?;
                adopt_check(&current.key.to_string(), current.status, *active)?;
                let reason = format!("adopted {reference}");
                let mut chain = start_chain(stamp, &world.tasks);
                if current.status == TaskStatus::Ready {
                    chain = task_moved(
                        chain,
                        &world.stacks,
                        stamp,
                        task_id,
                        TaskStatus::Claimed,
                        &reason,
                    )?;
                    chain = claimed(chain, task_id, &actor);
                }
                let holder = chain
                    .tasks
                    .task(task_id)
                    .ok_or_else(|| gone(task_id))?
                    .clone();
                let config: Map<String, serde_json::Value> = [
                    ("external", json!(true)),
                    ("adopted", json!(true)),
                    (VERIFIER_PID_KEY, json!(std::process::id())),
                    ("ref", json!(reference)),
                    ("repository", json!(repo.name)),
                    ("baseline_ref", json!(base_ref)),
                    ("baseline_sha", json!(baseline)),
                ]
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value))
                .collect();
                let (chain, row) = created(
                    chain,
                    stamp,
                    &holder,
                    next_attempt_number(numbers),
                    NewAttempt {
                        provider: "external".to_owned(),
                        adapter: "external".to_owned(),
                        status: AttemptStatus::Queued,
                        agent_id: Some(actor.clone()),
                        model: None,
                        session_id: None,
                        parent_attempt_id: None,
                        config_snapshot: config,
                    },
                );
                let (chain, row) = status_set(chain, stamp, &row, AttemptStatus::Running)?;
                let (set_id, ids) = chain.ids.next();
                let (item_id, ids) = ids.next();
                let set = ChangeSet {
                    id: set_id,
                    workspace_id: task.workspace_id,
                    task_id,
                    attempt_id: Some(row.id),
                    title: reason.clone(),
                    status: ChangeSetStatus::Open,
                    created_at: stamp.at,
                    updated_at: stamp.at,
                };
                let item = ChangeSetItem {
                    id: item_id,
                    change_set_id: set_id,
                    repository_id: repo.id,
                    base_sha: Some(baseline.clone()),
                    head_sha: Some(head.clone()),
                    branch: None,
                    commit_shas: commits
                        .iter()
                        .cloned()
                        .fold(VectorSync::new_sync(), |list, sha| list.push_back(sha)),
                    status: ChangeSetItemStatus::Committed,
                    detail: None,
                    updated_at: stamp.at,
                };
                let chain = Applied {
                    decision: chain
                        .decision
                        .write(Write::insert(set))
                        .write(Write::insert(item)),
                    ids,
                    tasks: chain.tasks,
                };
                let chain = task_moved(
                    chain,
                    &world.stacks,
                    stamp,
                    task_id,
                    TaskStatus::AwaitingVerification,
                    &reason,
                )?;
                Ok(chain.decision.with_reply((row.id, row.number)))
            },
        )?
        .reply;
    // The gate needs the adopted ref checked out in a managed worktree, as a submit's attempt has.
    // The read-only request plans no change set, so the adopted change set above stays the only one.
    let prepared = worktree_prepare(
        app,
        context,
        attempt_id,
        Some(vec![PrepareRequest {
            repository_id: repo.id,
            access: "read_only".to_owned(),
            target_ref: Some(head.clone()),
            base: None,
        }]),
        Some("worktree"),
        &[],
    );
    if let Err(err) = prepared {
        let end = EarlyEnd::external_start_failed(&err.message);
        let _ = finalize_failure(app, context, attempt_id, &end);
        return Err(err);
    }
    let summary = format!("adopted {reference}");
    let verification = gate_adopted(app, context, task_id, attempt_id, &summary)?;
    worktree_release(app, context, attempt_id, false, false)?;
    let facts = app
        .runtime
        .store
        .read(|session| load_submit(session, &attempt_id.to_string()))?;
    Ok(json_value(&json!({
        "attempt_id": attempt_id.to_string(),
        "attempt_number": number,
        "task": task.key.to_string(),
        "repository": repo.name,
        "head_sha": head,
        "baseline_sha": baseline,
        "baseline_ref": base_ref,
        "commits": commits.len(),
        "verification": verification,
        "task_status": facts.task.status.as_str(),
        "attempt_status": facts.attempt.status.as_str(),
    })))
}
