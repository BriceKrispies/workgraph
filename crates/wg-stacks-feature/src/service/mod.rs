//! The stack services (IF-53): the entry points behind `stack create`, `show`, `list`, `add`,
//! `remove`, `readmit`, `integrate`, `verify`, `history` and `eject`, the `/stacks` routes and the
//! stack tools (`StackService`, `StackVerifier.request`).
//!
//! Each one resolves the workspace, loads the task world with the stacks' repository rows in the
//! transaction of the call, asks git what only git knows (the base commits of a new stack, the
//! tips of the integration worktrees, the tip of an attempt branch), hands it all to the pure
//! function of [`crate::spec`] and applies what it returns. `integrate` and `verify` only queue
//! jobs; the worker that runs them is the feature's job handlers ([`crate::jobs`]).
//!
//! * [`reads`]: the `Session` readers of the stack rows;
//! * [`exec`]: the executor, the `stack_integrate` and `stack_verify` jobs, `eject` and landing.

pub mod exec;
pub mod reads;

use std::path::{Path, PathBuf};

use wg_git::{Git, RunOptions};
use wg_json::Value;
use wg_model::Row;
use wg_model::slice::QueueSlice;
use wg_registry::checkout::primary_checkout;
use wg_runtime::{App, RequestContext, index_written};
use wg_runtimespec::effects::GIT_TIMEOUT_MS;
use wg_store::{EventQuery, Session, TaskWorld};
use wg_types::{Id, Permission, WgError};

use crate::spec::git_failure;
use crate::spec::out::Tips;
use crate::spec::reads::{self as pure_reads, TipTarget};
use crate::spec::writes::{self, Queued, RecordedBranch};

/// The task world of `scope` with the repository rows of its stacks, which the world's own
/// statements do not load.
pub fn load_world(session: &mut Session<'_>, scope: Id) -> Result<TaskWorld, WgError> {
    let world = session.task_world(Some(scope), &[])?;
    let stack_ids: Vec<Id> = world.stacks.stacks.keys().copied().collect();
    let mut rows = Vec::new();
    for stack_id in stack_ids {
        rows.extend(
            reads::stack_repositories(session, stack_id)?
                .into_iter()
                .map(Row::from),
        );
    }
    Ok(world.with_rows(rows))
}

/// `StackIntegrator.integration_tip` for each target: the head of its worktree, when that exists.
fn read_tips(worktrees: &Path, targets: &[TipTarget]) -> Tips {
    targets
        .iter()
        .filter_map(|target| {
            let path = worktrees.join(&target.directory);
            if std::fs::metadata(&path).is_err() {
                return None;
            }
            let head = Git {
                path,
                timeout_ms: GIT_TIMEOUT_MS,
            }
            .head()
            .ok()
            .flatten()?;
            Some(((target.stack_id, target.repository.clone()), head))
        })
        .collect()
}

/// `latest_event_id`: the newest event about the stack, which versions its jobs; the stack's own
/// id before it has any.
fn latest_event_id(session: &mut Session<'_>, stack_id: Id) -> Result<Id, WgError> {
    let newest = session.query_events(&EventQuery {
        entity_ids: vec![stack_id],
        descending: true,
        limit: 1,
        ..EventQuery::default()
    })?;
    Ok(newest.first().map_or(stack_id, |event| event.id))
}

/// The jobs already stored under `keys` (what an enqueue dedupes against).
fn jobs_under(session: &mut Session<'_>, keys: &[String]) -> Result<QueueSlice, WgError> {
    let mut slice = QueueSlice::default();
    for key in keys {
        for job in session.enqueue_slice(Some(key))?.jobs.values() {
            slice.jobs = slice.jobs.insert(job.id, job.clone());
        }
    }
    Ok(slice)
}

/// The queued stack of a request with the version of its state and the jobs under `keys`.
type Pending = (Queued, Id, QueueSlice);

/// A stack-write check against a fresh stamp, before anything is looked up.
pub fn require_stack_write(app: &mut App, context: &RequestContext) -> Result<(), WgError> {
    app.runtime
        .stamps
        .stamp(context)?
        .require(&[Permission::StackWrite])
}

/// `StackService.list`: the workspace's stacks, newest first, with their integration tips.
pub fn list(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    include_inactive: bool,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    let worktrees = PathBuf::from(app.paths.worktrees_dir().as_str());
    app.runtime.query(
        |session| {
            let world = load_world(session, scope)?;
            let tips = read_tips(
                &worktrees,
                &pure_reads::list_targets(&world, scope, include_inactive),
            );
            Ok((world, tips))
        },
        |(world, tips)| pure_reads::list(world, scope, include_inactive, tips),
    )
}

/// `StackService.get`: the parent's newest stack.
pub fn show(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    let worktrees = PathBuf::from(app.paths.worktrees_dir().as_str());
    app.runtime.query(
        |session| {
            let world = load_world(session, scope)?;
            let tips = read_tips(
                &worktrees,
                &pure_reads::show_targets(&world, scope, parent)?,
            );
            Ok((world, tips))
        },
        |(world, tips)| pure_reads::show(world, scope, parent, tips),
    )
}

/// `StackService.containing`: every stack the task is an item of, newest first.
pub fn containing(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task: &str,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| load_world(session, scope),
        |world| pure_reads::containing(world, scope, task),
    )
}

/// `StackService.history`: the parent's newest stack's events, with the verification runs they
/// name.
pub fn history(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| {
            let world = load_world(session, scope)?;
            let stack_id = pure_reads::history_stack(&world, scope, parent)?;
            let events = session.query_events(&EventQuery {
                entity_type: Some("stack".to_owned()),
                entity_ids: vec![stack_id],
                limit: 10_000,
                ..EventQuery::default()
            })?;
            let run_ids = pure_reads::run_ids(&events);
            let runs = if run_ids.is_empty() {
                Vec::new()
            } else {
                session.query_events(&EventQuery {
                    entity_type: Some("verification_run".to_owned()),
                    entity_ids: run_ids,
                    limit: 10_000,
                    ..EventQuery::default()
                })?
            };
            Ok((events, runs))
        },
        |(events, runs)| Ok(pure_reads::history_out(events.clone(), runs.clone())),
    )
}

/// `StackService.create`: a stack of the parent's children, based on each repository's default
/// branch head now.
pub fn create(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
) -> Result<Value, WgError> {
    require_stack_write(app, context)?;
    let scope = app.workspace_or_default(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| {
                let world = load_world(session, scope)?;
                let mut shas = Vec::new();
                for repository in writes::create_plan(&world, scope, parent)?.repositories {
                    let checkouts = session.checkouts(repository.id)?;
                    let (path, branch) = writes::base_source(repository, &checkouts)?;
                    let sha = Git {
                        path: PathBuf::from(&path),
                        timeout_ms: GIT_TIMEOUT_MS,
                    }
                    .resolve_commit(&branch)
                    .map_err(|err| git_failure(&err))?;
                    shas.push((repository.name.clone(), sha));
                }
                Ok((world, shas))
            },
            |(world, shas), stamp| writes::create(world, stamp, scope, parent, shas),
            index_written,
        )?
        .reply)
}

/// `StackService.add_item`: a child of the parent joins its active stack.
pub fn add(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
    task: &str,
) -> Result<Value, WgError> {
    require_stack_write(app, context)?;
    let scope = app.workspace_or_default(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| load_world(session, scope),
            |world, stamp| writes::add_item(world, stamp, scope, parent, task),
            index_written,
        )?
        .reply)
}

/// `StackService.remove_item`: a child that is not on the integration branch leaves the stack.
pub fn remove(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
    task: &str,
) -> Result<Value, WgError> {
    require_stack_write(app, context)?;
    let scope = app.workspace_or_default(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| load_world(session, scope),
            |world, stamp| writes::remove_item(world, stamp, scope, parent, task),
            index_written,
        )?
        .reply)
}

/// `StackService.readmit_item`: an ejected or pending child is queued for integration again.
pub fn readmit(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
    task: &str,
) -> Result<Value, WgError> {
    require_stack_write(app, context)?;
    let scope = app.workspace_or_default(workspace)?;
    Ok(app
        .runtime
        .decide_then(
            context,
            |session| {
                let world = load_world(session, scope)?;
                let (child_id, key, probes) = writes::readmit_probes(&world, scope, parent, task)?;
                let mut recorded = Vec::new();
                for probe in probes {
                    let Some(head) = session.latest_recorded_head(child_id, probe.repository_id)?
                    else {
                        continue;
                    };
                    let Some(branch) = head.branch else {
                        continue;
                    };
                    let checkouts = session.checkouts(probe.repository_id)?;
                    let checkout = primary_checkout(&checkouts, true)
                        .ok_or_else(|| writes::no_checkout(&probe.name))?;
                    // Branches are shared by every worktree of a repository, so the primary
                    // checkout reads the tip.
                    let found = Git {
                        path: PathBuf::from(&checkout.path),
                        timeout_ms: GIT_TIMEOUT_MS,
                    }
                    .run_with(
                        &[
                            "rev-parse",
                            "--verify",
                            "--quiet",
                            &format!("refs/heads/{branch}"),
                        ],
                        RunOptions::UNCHECKED,
                    )
                    .map_err(|err| git_failure(&err))?;
                    let entry = RecordedBranch {
                        repository: probe.name,
                        branch,
                        head: head.head_sha,
                        tip: writes::branch_tip(&found.stdout),
                    };
                    writes::check_recorded(&key, std::slice::from_ref(&entry))?;
                    recorded.push(entry);
                }
                Ok((world, recorded))
            },
            |(world, recorded), stamp| writes::readmit(world, stamp, scope, parent, task, recorded),
            index_written,
        )?
        .reply)
}

/// `StackService.integrate`: one `stack_integrate` job per repository of the active stack.
pub fn integrate(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
) -> Result<Value, WgError> {
    require_stack_write(app, context)?;
    let scope = app.workspace_or_default(workspace)?;
    Ok(app
        .runtime
        .decide(
            context,
            |session| -> Result<Pending, WgError> {
                let world = load_world(session, scope)?;
                let queued = writes::queued(&world, scope, parent)?;
                let version = latest_event_id(session, queued.stack.id)?;
                let jobs = jobs_under(session, &writes::integration_keys(&queued, version))?;
                Ok((queued, version, jobs))
            },
            |(queued, version, jobs), stamp| writes::integrate(stamp, queued, *version, jobs),
        )?
        .reply)
}

/// `StackVerifier.request`: one `stack_verify` job for the active stack's integration tips.
pub fn verify(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    parent: &str,
) -> Result<Value, WgError> {
    require_stack_write(app, context)?;
    let scope = app.workspace_or_default(workspace)?;
    let worktrees = PathBuf::from(app.paths.worktrees_dir().as_str());
    Ok(app
        .runtime
        .decide(
            context,
            |session| -> Result<(Pending, Vec<(String, String)>), WgError> {
                let world = load_world(session, scope)?;
                let queued = writes::queued(&world, scope, parent)?;
                let version = latest_event_id(session, queued.stack.id)?;
                let found = read_tips(&worktrees, &writes::verification_targets(&queued));
                let tips = writes::tips_or_bases(&queued, &found);
                let key = writes::verification_key(&queued, &tips, version);
                let jobs = jobs_under(session, &[key])?;
                Ok(((queued, version, jobs), tips))
            },
            |((queued, version, jobs), tips), stamp| {
                writes::request_verification(stamp, queued, tips, *version, jobs)
            },
        )?
        .reply)
}
