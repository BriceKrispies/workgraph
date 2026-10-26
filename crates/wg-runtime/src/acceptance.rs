//! The acceptance entry points of the application (IF-24): the services behind `task accept`,
//! `task reject`, `task promote-dependents` and the acceptance list, and the landing of a
//! verified stack when its parent is accepted.
//!
//! The decisions are `wg_runtimespec::tasks::acceptance` and `landing`; this module loads what
//! they read, runs the git commands of a landing (the fast-forward of each default branch with
//! the rollback plan of `wg_stacks::landing`, and the removal of the integration worktrees and
//! branches) and applies the result.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wg_git::{Git, GitError, RunOptions};
use wg_jobs::queue::JobFilter;
use wg_json::Value;
use wg_model::RowKey;
use wg_model::entity::{AgentAttempt, Repository, Stack, VerificationRun};
use wg_registry::checkout::is_within;
use wg_registry::resolve;
use wg_runtimespec::RequestContext;
use wg_runtimespec::tasks::acceptance::{
    AcceptFacts, AcceptWorld, Route, accept_task, acceptances_out, item_stack_id, land_request_of,
    promote_dependents, rebuild_unavailable, reject_task, with_promoted,
};
use wg_runtimespec::tasks::landing::{
    Heads, LandWorld, RepoRows, Target, directory_name, finish_landing, integration_busy_of,
    integration_removed, land_state, move_dir, native_path, refusal_event, targets, verified_from,
    worktree_removed,
};
use wg_stacks::landing::{
    FfCheck, FfMove, LandDecision, fast_forward_plan, guard_defer, land_decision, move_args,
    moved_while_landing, rollback_args,
};
use wg_store::{EventQuery, Session, TaskWorld};
use wg_types::{AttemptStatus, ErrorCode, Id, JobStatus, Permission, WgError, WorktreeStatus};

use crate::app::App;
use crate::tasks::index_written;

/// A git failure as the domain error (`GitError` in Python).
fn git_failure(error: GitError) -> WgError {
    WgError::new(ErrorCode::GitError, error.to_string())
}

/// The runner for a repository or checkout at `$path`, with the default timeout.
macro_rules! runner {
    ($path:expr) => {
        Git {
            path: PathBuf::from($path),
            timeout_ms: wg_runtimespec::effects::GIT_TIMEOUT_MS,
        }
    };
}

/// The latest succeeded attempt of `task` (`_latest_succeeded_attempt`).
fn latest_succeeded(session: &mut Session<'_>, task: Id) -> Result<Option<AgentAttempt>, WgError> {
    Ok(session
        .attempts_of_task(task, &[AttemptStatus::Succeeded])?
        .into_iter()
        .max_by_key(|attempt| attempt.number))
}

/// Integration work for `stack` is queued or running.
fn integration_busy(session: &mut Session<'_>, stack: Id) -> Result<bool, WgError> {
    let jobs = ["stack_integrate", "stack_verify"]
        .into_iter()
        .map(|kind| {
            session
                .list_jobs(&JobFilter {
                    statuses: &[JobStatus::Queued, JobStatus::Running],
                    kind: Some(kind),
                    ..JobFilter::default()
                })
                .map(|(found, _)| found)
        })
        .collect::<Result<Vec<_>, _>>()?
        .concat();
    Ok(integration_busy_of(&jobs, stack))
}

/// The facts of an acceptance of `task`: its latest run, that run's manifest and its latest
/// succeeded attempt.
fn facts_of(session: &mut Session<'_>, task: Id) -> Result<AcceptFacts, WgError> {
    let run = session.latest_verification_run(task)?;
    let manifest = match run.as_ref().and_then(|found| found.manifest_id) {
        Some(id) => session.manifest_by_id(id)?,
        None => None,
    };
    Ok(AcceptFacts {
        run,
        manifest,
        attempt: latest_succeeded(session, task)?,
    })
}

/// What `accept` reads for the task `reference` names.
fn load_accept(
    session: &mut Session<'_>,
    scope: Option<Id>,
    reference: &str,
) -> Result<AcceptWorld, WgError> {
    let world = session.task_world(scope, &[])?;
    let Some((task, workspace)) = resolve::task(&world.tasks, scope, reference)
        .ok()
        .map(|found| (found.id, found.workspace_id))
    else {
        return Ok(AcceptWorld {
            world,
            ..AcceptWorld::default()
        });
    };
    let busy = match item_stack_id(&world.stacks, task) {
        Some(stack) => integration_busy(session, stack)?,
        None => false,
    };
    Ok(AcceptWorld {
        configs: session.verification_configs(workspace)?,
        facts: facts_of(session, task)?,
        busy,
        world,
    })
}

/// What a landing reads before it moves anything.
struct Landing {
    world: TaskWorld,
    stack: Stack,
    rows: Vec<RepoRows>,
    verified: bool,
    run_id: Option<Id>,
    tips: BTreeMap<String, String>,
    busy: bool,
}

fn load_landing(session: &mut Session<'_>, stack_id: Id) -> Result<Landing, WgError> {
    let stack = session
        .stack_by_id(stack_id)?
        .ok_or_else(|| WgError::new(ErrorCode::NotFound, format!("stack {stack_id} not found")))?;
    let world = session.task_world(Some(stack.workspace_id), &[])?;
    let rows = session
        .stack_repositories(stack_id)?
        .into_iter()
        .map(|link| {
            let repository = session
                .fetch::<Repository>(&RowKey::Repository(link.repository_id))?
                .ok_or_else(|| {
                    WgError::new(
                        ErrorCode::NotFound,
                        format!("stack {stack_id} has no repository {}", link.repository_id),
                    )
                })?;
            Ok(RepoRows {
                checkouts: session.checkouts(link.repository_id)?,
                repository,
                link,
            })
        })
        .collect::<Result<Vec<_>, WgError>>()?;
    let event = session
        .query_events(&EventQuery {
            entity_ids: vec![stack_id],
            event_types: vec!["stack.verified".to_owned()],
            limit: 1,
            descending: true,
            ..EventQuery::default()
        })?
        .into_iter()
        .next();
    let (run_id, tips) = event
        .as_ref()
        .map(|found| verified_from(&found.data))
        .unwrap_or_default();
    Ok(Landing {
        busy: integration_busy(session, stack_id)?,
        verified: event.is_some(),
        world,
        stack,
        rows,
        run_id,
        tips,
    })
}

/// What `_finish` reads, inside the transaction that writes.
fn load_finish(
    session: &mut Session<'_>,
    stack_id: Id,
    run_id: Option<Id>,
) -> Result<LandWorld, WgError> {
    let stack = session
        .stack_by_id(stack_id)?
        .ok_or_else(|| WgError::new(ErrorCode::NotFound, format!("stack {stack_id} not found")))?;
    let world = session.task_world(Some(stack.workspace_id), &[])?;
    let run = match run_id {
        Some(id) => session.fetch::<VerificationRun>(&RowKey::VerificationRun(id))?,
        None => None,
    };
    let manifest = match run.as_ref().and_then(|found| found.manifest_id) {
        Some(id) => session.manifest_by_id(id)?,
        None => None,
    };
    let tasks: Vec<Id> = world
        .stacks
        .items_of(stack_id)
        .map(|item| item.task_id)
        .chain([stack.parent_task_id])
        .collect();
    let attempts = tasks
        .into_iter()
        .map(|task| latest_succeeded(session, task).map(|found| found.map(|a| (task, a))))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    Ok(LandWorld {
        world,
        stack_id,
        run_id,
        run,
        manifest,
        attempts,
    })
}

/// What `_fast_forward` reads from git for one repository.
fn ff_check(target: &Target, tip: &str) -> Result<FfCheck, WgError> {
    let source = runner!(&target.source_path);
    let default = target.default_branch.clone().unwrap_or_default();
    let descends = source
        .is_ancestor(&target.base_sha, tip)
        .map_err(git_failure)?;
    let holder = source
        .worktrees()
        .map_err(git_failure)?
        .into_iter()
        .find(|entry| {
            entry.branch.as_deref() == Some(default.as_str())
                && Path::new(&entry.path).exists()
                && !entry.prunable
        })
        .map(|entry| {
            let clean = runner!(&entry.path).is_clean();
            (native_path(&entry.path), clean)
        });
    let holder = holder
        .map(|(path, clean)| clean.map(|clean| (path, clean)).map_err(git_failure))
        .transpose()?;
    Ok(FfCheck {
        repository: target.repository_name.clone(),
        default_branch: default,
        base_sha: target.base_sha.clone(),
        tip: tip.to_owned(),
        descends,
        holder,
    })
}

/// Makes the planned moves in order; a move that fails undoes the ones already made, newest
/// first, and is reported as a default branch that moved under the landing.
fn make_moves(moves: &[(FfMove, String)]) -> Result<(), WgError> {
    let mut done: Vec<&(FfMove, String)> = Vec::new();
    for entry in moves {
        let (step, source) = entry;
        let args = move_args(step);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        match runner!(move_dir(step, source)).run_with(&refs, RunOptions::CHECKED) {
            Ok(_) => done.push(entry),
            Err(_) => {
                for (undo, home) in done.iter().rev().map(|entry| (&entry.0, &entry.1)) {
                    let args = rollback_args(undo);
                    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                    let _ = runner!(move_dir(undo, home)).run_with(&refs, RunOptions::UNCHECKED);
                }
                return Err(moved_while_landing());
            }
        }
    }
    Ok(())
}

impl App {
    /// `VerificationService.accept`: automated acceptance. The parent of a stack lands the
    /// stack instead.
    pub fn task_accept(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        reason: &str,
    ) -> Result<Value, WgError> {
        self.accept_with(context, workspace, reference, false, reason)
    }

    /// `VerificationService.accept_manually`: the acceptance of a reviewer, with a reason.
    pub fn task_accept_manually(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        reason: &str,
    ) -> Result<Value, WgError> {
        self.accept_with(context, workspace, reference, true, reason)
    }

    fn accept_with(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        manual: bool,
        reason: &str,
    ) -> Result<Value, WgError> {
        self.require_permission(context, Permission::TaskAccept)?;
        let scope = self.workspace_or_all(workspace)?;
        let route = self
            .runtime
            .decide_then(
                context,
                |session| load_accept(session, scope, reference),
                |loaded: &AcceptWorld, stamp| {
                    accept_task(loaded, stamp, scope, reference, manual, reason)
                },
                index_written,
            )?
            .reply;
        match route {
            Route::Done(out) => Ok(out),
            Route::Land { stack_id } => {
                let (mode, text) = land_request_of(manual, reason);
                self.land_stack(context, stack_id, mode, &text, true)
            }
        }
    }

    /// `VerificationService.reject`.
    pub fn task_reject(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        reason: &str,
    ) -> Result<Value, WgError> {
        self.require_permission(context, Permission::TaskAccept)?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
            .runtime
            .decide_then(
                context,
                |session| session.task_world(scope, &[]),
                |world: &TaskWorld, stamp| reject_task(world, stamp, scope, reference, reason),
                index_written,
            )?
            .reply)
    }

    /// `TaskService.promote_dependents`: the keys of the dependents moved to `ready`.
    pub fn task_promote_dependents(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
    ) -> Result<Value, WgError> {
        self.require_permission(context, Permission::TaskWrite)?;
        let scope = self.workspace_or_all(workspace)?;
        Ok(self
            .runtime
            .decide_then(
                context,
                |session| session.task_world(scope, &[]),
                |world: &TaskWorld, stamp| promote_dependents(world, stamp, scope, reference),
                index_written,
            )?
            .reply)
    }

    /// `accept --promote-dependents`: the acceptance and the keys it promoted.
    pub fn task_accept_promoting(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        manual: bool,
        reason: &str,
    ) -> Result<Value, WgError> {
        let out = self.accept_with(context, workspace, reference, manual, reason)?;
        let promoted = self.task_promote_dependents(context, workspace, reference)?;
        Ok(with_promoted(&out, &promoted))
    }

    /// `VerificationService.acceptances`: the decisions recorded for a task, oldest first.
    pub fn task_acceptances(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
    ) -> Result<Value, WgError> {
        self.require_read(context)?;
        let scope = self.workspace_or_all(workspace)?;
        self.runtime.query(
            |session| {
                let world = session.task_world(scope, &[])?;
                let task = resolve::task(&world.tasks, scope, reference)?.id;
                session.task_acceptances(task)
            },
            |rows| Ok(acceptances_out(rows)),
        )
    }

    /// `StackLander.land`: accepts a verified stack's parent and lands its children. A refusal
    /// is recorded as a `stack.landing_refused` event and raised.
    pub fn land_stack(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        mode: &str,
        reason: &str,
        guard: bool,
    ) -> Result<Value, WgError> {
        match self.try_land(context, stack_id, mode, reason, guard) {
            Err(error)
                if matches!(
                    error.code,
                    ErrorCode::Conflict | ErrorCode::ValidationFailed
                ) =>
            {
                self.runtime.decide(
                    context,
                    |session| session.stack_by_id(stack_id),
                    |stack, stamp| {
                        Ok(stack.as_ref().map_or_else(
                            || wg_model::Decision::new(()),
                            |found| refusal_event(stamp, found, mode, reason, &error),
                        ))
                    },
                )?;
                Err(error)
            }
            other => other,
        }
    }

    fn try_land(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        mode: &str,
        reason: &str,
        guard: bool,
    ) -> Result<Value, WgError> {
        let loaded = self
            .runtime
            .store
            .read(|session| load_landing(session, stack_id))?;
        let state = |targets: &[Target], heads: &[Heads]| {
            land_state(
                &loaded.world,
                &loaded.stack,
                loaded.verified,
                &loaded.tips,
                targets,
                heads,
            )
        };
        land_decision(&state(&[], &[])?).map_err(|refusal| refusal.to_error())?;
        let found = targets(&loaded.rows)?;
        let integrated = found
            .iter()
            .map(|target| match target.default_branch.as_deref() {
                Some(branch) if !branch.is_empty() => runner!(&target.source_path)
                    .resolve_commit(&target.integration_branch)
                    .map_err(git_failure),
                _ => Ok(String::new()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let provisional: Vec<Heads> = found
            .iter()
            .zip(&integrated)
            .map(|(target, tip)| Heads {
                integrated: tip.clone(),
                default: target.base_sha.clone(),
            })
            .collect();
        land_decision(&state(&found, &provisional)?).map_err(|refusal| refusal.to_error())?;
        let heads = found
            .iter()
            .zip(&integrated)
            .map(|(target, tip)| {
                let default = runner!(&target.source_path)
                    .resolve_commit(target.default_branch.as_deref().unwrap_or_default())
                    .map_err(git_failure)?;
                Ok(Heads {
                    integrated: tip.clone(),
                    default,
                })
            })
            .collect::<Result<Vec<_>, WgError>>()?;
        match land_decision(&state(&found, &heads)?).map_err(|refusal| refusal.to_error())? {
            LandDecision::Deferred { .. } => {
                if guard {
                    guard_defer(&stack_id.to_string(), loaded.busy)?;
                }
                Err(rebuild_unavailable(
                    "landing on a default branch that moved since the stack was cut",
                ))
            }
            LandDecision::Land { .. } => {
                let tips: Vec<(String, String)> = found
                    .iter()
                    .map(|target| {
                        (
                            target.repository_name.clone(),
                            loaded
                                .tips
                                .get(&target.repository_name)
                                .cloned()
                                .unwrap_or_default(),
                        )
                    })
                    .collect();
                let checks = found
                    .iter()
                    .zip(&tips)
                    .map(|(target, (_, tip))| ff_check(target, tip))
                    .collect::<Result<Vec<_>, _>>()?;
                let moves: Vec<(FfMove, String)> = fast_forward_plan(&checks)?
                    .into_iter()
                    .zip(&found)
                    .map(|(step, target)| (step, target.source_path.clone()))
                    .collect();
                make_moves(&moves)?;
                let run_id = loaded.run_id;
                let landed = self.runtime.decide_then(
                    context,
                    |session| load_finish(session, stack_id, run_id),
                    |world: &LandWorld, stamp| finish_landing(world, stamp, &tips, mode, reason),
                    index_written,
                )?;
                self.remove_integration(context, stack_id, &found)?;
                Ok(landed.reply)
            }
        }
    }

    /// `StackLander._remove_integration`: removes the integration worktree and branch of each
    /// repository and records it.
    fn remove_integration(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        found: &[Target],
    ) -> Result<(), WgError> {
        let root = self.paths.worktrees_dir().as_str().to_owned();
        found.iter().try_for_each(|target| {
            let path = Path::new(&root)
                .join("stacks")
                .join(stack_id.to_string())
                .join(directory_name(&target.repository_name));
            let text = path.to_string_lossy().into_owned();
            let rows = self
                .runtime
                .store
                .read(|session| session.worktrees_with_status(&[WorktreeStatus::Active]))?;
            rows.into_iter()
                .filter(|row| row.path == text)
                .try_for_each(|row| self.remove_worktree(context, &row, &root, target))?;
            let _ = runner!(&target.source_path).run_with(
                &["branch", "-D", &target.integration_branch],
                RunOptions::UNCHECKED,
            );
            self.runtime.decide(
                context,
                |session| session.stack_by_id(stack_id),
                |stack, stamp| {
                    Ok(stack.as_ref().map_or_else(
                        || wg_model::Decision::new(()),
                        |found| {
                            integration_removed(
                                stamp,
                                found,
                                &target.repository_name,
                                &target.integration_branch,
                            )
                        },
                    ))
                },
            )?;
            Ok(())
        })
    }

    /// `WorktreeService.remove(force=True)`.
    fn remove_worktree(
        &mut self,
        context: &RequestContext,
        row: &wg_model::entity::ManagedWorktree,
        root: &str,
        target: &Target,
    ) -> Result<(), WgError> {
        if !is_within(&row.path, root) {
            return Err(WgError::new(
                ErrorCode::ValidationFailed,
                format!(
                    "refusing to remove {}: not inside Workgraph's worktree directory",
                    row.path
                ),
            ));
        }
        let path = Path::new(&row.path);
        let source = runner!(&target.source_path);
        let is_repository = source.is_repository().unwrap_or(false);
        if path.exists() {
            if is_repository {
                let removed = source.run_with(
                    &["worktree", "remove", "--force", &row.path],
                    RunOptions::UNCHECKED,
                );
                if removed.map_or(true, |out| out.returncode != 0) && path.exists() {
                    return Ok(());
                }
            } else {
                let _ = std::fs::remove_dir_all(path);
            }
        }
        if is_repository {
            let _ = source.run_with(&["worktree", "prune"], RunOptions::UNCHECKED);
        }
        self.runtime.decide(
            context,
            |_| Ok(()),
            |(), stamp| Ok(worktree_removed(stamp, row)),
        )?;
        if let Some(parent) = path.parent() {
            let bare = parent.to_string_lossy().into_owned();
            let empty = parent.exists()
                && is_within(&bare, root)
                && bare != root
                && std::fs::read_dir(parent)
                    .map(|entries| {
                        entries
                            .flatten()
                            .all(|entry| entry.file_name() == "workgraph-manifest.json")
                    })
                    .unwrap_or(false);
            if empty {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
        Ok(())
    }
}
