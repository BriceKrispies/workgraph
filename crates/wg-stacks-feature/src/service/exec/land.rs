//! `StackLander`: landing a verified stack on the default branches, rebasing a stack whose
//! default branch moved, and rebuilding the integration branches after a release.

use std::collections::BTreeMap;
use std::path::Path;

use crate::spec::exec::land::{
    AcceptFacts, GitFacts, LandBase, Moved, decide, finish_landing, integration_removed,
    is_refusal, land_base, landing_refused, move_bases, parse_land_request, precheck, verified,
};
use crate::spec::exec::worktree::{IntegrationTarget, integration_path};
use serde_json::Value;
use wg_git::RunOptions;
use wg_model::RowKey;
use wg_model::entity::{AgentAttempt, RevisionManifest, VerificationRun};
use wg_runtimespec::RequestContext;
use wg_stacks::landing::{
    FfCheck, FfStep, LandDecision, LandRepo, after_verified_outcome, after_verified_request,
    fast_forward_plan, guard_defer, move_args, moved_while_landing, rollback_args, rollback_order,
    stale_heads,
};
use wg_stacks::verify::StackRepoBase;
use wg_store::EventQuery;
use wg_types::{AttemptStatus, Id, WgError, WorktreeStatus};

use super::stack_world;

use crate::spec::{git_failure, native_root};
use wg_runtime::index_written;

use super::Exec;

impl Exec<'_> {
    /// `_targets`: the integration target of every repository of the stack, by repository name.
    fn stack_targets(&mut self, stack_id: Id) -> Result<Vec<IntegrationTarget>, WgError> {
        let repositories: Vec<Id> = self.0.runtime.query(
            |session| stack_world(session, stack_id),
            |world| {
                Ok(world
                    .stacks
                    .repositories_of(stack_id)
                    .map(|link| link.repository_id)
                    .collect())
            },
        )?;
        let mut targets = Vec::new();
        for repository_id in repositories {
            let target = self.stack_target(stack_id, repository_id)?.ok_or_else(|| {
                WgError::conflict(format!("stack {stack_id} is no longer active"))
                    .detail("stack_id", stack_id.to_string())
            })?;
            targets.push(target);
        }
        targets.sort_by(|a, b| a.repository_name.cmp(&b.repository_name));
        Ok(targets)
    }

    /// `rebase_if_stale`: the stack is rebuilt onto default branches that moved before it
    /// verified. Returns whether it was.
    pub(crate) fn rebase_if_stale(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
    ) -> Result<bool, WgError> {
        let targets = self.stack_targets(stack_id)?;
        let world = self.0.runtime.query(
            |session| stack_world(session, stack_id),
            |world| Ok(world.clone()),
        )?;
        let defaults: BTreeMap<Id, Option<String>> = targets
            .iter()
            .map(|target| {
                (
                    target.repository_id,
                    world
                        .tasks
                        .registry
                        .repository(target.repository_id)
                        .and_then(|repository| repository.default_branch.clone()),
                )
            })
            .collect();
        let mut repos: Vec<LandRepo> = Vec::new();
        for target in &targets {
            let default = defaults.get(&target.repository_id).cloned().flatten();
            let head = match default.as_deref().filter(|name| !name.is_empty()) {
                Some(name) => git_at!(Path::new(&target.source_path))
                    .resolve_commit(name)
                    .map_err(|err| git_failure(&err))?,
                None => String::new(),
            };
            repos.push(LandRepo {
                id: target.repository_id.to_string(),
                name: target.repository_name.clone(),
                default_branch: default.clone(),
                base_sha: target.base_sha.clone(),
                integrated_tip: String::new(),
                default_head: head,
            });
            if default.as_deref().is_none_or(str::is_empty) {
                break;
            }
        }
        let Some(heads) = stale_heads(&repos) else {
            return Ok(false);
        };
        self.0.runtime.decide(
            context,
            |session| stack_world(session, stack_id),
            |world, stamp| move_bases(world, stamp, stack_id, &heads, &Moved::Rebased),
        )?;
        self.rebuild_stack(context, stack_id, Some(&heads))?;
        Ok(true)
    }

    /// `StackLander.rebuild_without`: the integration branches are rebuilt from their current
    /// bases, after a release or an ejection.
    pub fn rebuild_without(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
    ) -> Result<(), WgError> {
        self.rebuild_stack(context, stack_id, None)
    }

    /// `_rebuild`.
    fn rebuild_stack(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        bases: Option<&BTreeMap<String, String>>,
    ) -> Result<(), WgError> {
        let targets = self.stack_targets(stack_id)?;
        let repos: Vec<StackRepoBase> = targets
            .iter()
            .map(|target| StackRepoBase {
                repository_id: target.repository_id.to_string(),
                name: target.repository_name.clone(),
                base_sha: wg_stacks::landing::rebuild_base(
                    &target.repository_name,
                    &target.base_sha,
                    bases,
                ),
            })
            .collect();
        self.rebuild_branches(context, stack_id, &targets, &repos)?;
        self.enqueue_if_settled(context, stack_id)?;
        Ok(())
    }

    /// `after_verified`: lands a stack that just verified, when a landing was requested or the
    /// workspace auto-accepts. Returns `landed`, `deferred`, `refused`, or `None` when nothing was
    /// asked for.
    pub(crate) fn after_verified(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
    ) -> Result<Option<&'static str>, WgError> {
        let state = self.0.runtime.query(
            |session| stack_world(session, stack_id),
            |world| {
                Ok(world
                    .stacks
                    .stack(stack_id)
                    .map(|stack| (stack.status, stack.land_request.clone(), stack.workspace_id)))
            },
        )?;
        let Some((status, request, workspace)) = state else {
            return Ok(None);
        };
        if status != wg_types::StackStatus::Verified {
            return Ok(None);
        }
        let stored = request.as_deref().and_then(parse_land_request);
        let auto_accept = self.stacks_auto_accept(workspace)?;
        let Some(asked) = after_verified_request(stored, auto_accept) else {
            return Ok(None);
        };
        let attempt = self.land_stack(context, stack_id, &asked.mode, &asked.reason, false);
        if let Err(err) = &attempt
            && !is_refusal(err.code)
        {
            return Err(err.clone());
        }
        Ok(Some(after_verified_outcome(&attempt).as_str()))
    }

    /// Whether the workspace's configuration lands verified stacks without a request.
    fn stacks_auto_accept(&mut self, workspace: Id) -> Result<bool, WgError> {
        let slug = self.0.workspace_slug(&workspace.to_string())?;
        let config = self.0.effective_config(Some(&slug))?;
        Ok(config
            .get("stacks")
            .and_then(|stacks| stacks.get("auto_accept"))
            .and_then(Value::as_bool)
            .unwrap_or(false))
    }

    /// `StackLander.land`: accepts a verified stack's parent and lands its children. Returns the
    /// parent's acceptance, or `None` when the default branch moved and the stack is being
    /// rebuilt to land once it verifies again. A refusal is recorded as an event and returned.
    pub fn land_stack(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        mode: &str,
        reason: &str,
        guard: bool,
    ) -> Result<Option<Id>, WgError> {
        match self.land_inner(context, stack_id, mode, reason, guard) {
            Err(err) if is_refusal(err.code) => {
                self.0.runtime.decide(
                    context,
                    |session| stack_world(session, stack_id),
                    |world, stamp| {
                        landing_refused(world, stamp, stack_id, mode, reason, &err.message)
                    },
                )?;
                Err(err)
            }
            other => other,
        }
    }

    fn land_inner(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        mode: &str,
        reason: &str,
        guard: bool,
    ) -> Result<Option<Id>, WgError> {
        let (world, event) = self.0.runtime.query(
            |session| {
                let world = stack_world(session, stack_id)?;
                let newest = session.query_events(&EventQuery {
                    entity_ids: vec![stack_id],
                    event_types: vec!["stack.verified".to_owned()],
                    descending: true,
                    limit: 1,
                    ..EventQuery::default()
                })?;
                Ok((world, newest.into_iter().next()))
            },
            |loaded| Ok(loaded.clone()),
        )?;
        let data = event.map(|record| Value::Object(record.data));
        let base: LandBase = land_base(&world, stack_id)?;
        precheck(&base, data.as_ref())?;
        let (run_id, tips) = verified(stack_id, data.as_ref())?;
        let targets = self.stack_targets(stack_id)?;
        let facts = self.git_facts(&base, &targets, &tips)?;
        match decide(&base, data.as_ref(), &facts)? {
            LandDecision::Deferred { .. } => {
                let heads: BTreeMap<String, String> = facts
                    .iter()
                    .map(|fact| (fact.repository.clone(), fact.default_head.clone()))
                    .collect();
                self.defer(context, stack_id, &heads, mode, reason, guard)?;
                Ok(None)
            }
            LandDecision::Land { .. } => {
                let plans = self.fast_forward_checks(&base, &targets, &tips)?;
                let sources: BTreeMap<String, String> = targets
                    .iter()
                    .map(|target| (target.repository_name.clone(), target.source_path.clone()))
                    .collect();
                self.fast_forward(&plans, &sources)?;
                let acceptance =
                    self.finish_landing_rows(context, stack_id, run_id, &tips, mode, reason)?;
                self.remove_integration(context, stack_id, &targets)?;
                Ok(Some(acceptance))
            }
        }
    }

    /// What git says about each repository, up to the first one a precondition refuses.
    fn git_facts(
        &self,
        base: &LandBase,
        targets: &[IntegrationTarget],
        tips: &BTreeMap<String, String>,
    ) -> Result<Vec<GitFacts>, WgError> {
        let mut facts = Vec::new();
        for target in targets {
            let default = base
                .repos
                .iter()
                .find(|repo| repo.name == target.repository_name)
                .and_then(|repo| repo.default_branch.clone())
                .filter(|name| !name.is_empty());
            let git = git_at!(Path::new(&target.source_path));
            let Some(default) = default else {
                facts.push(GitFacts {
                    repository: target.repository_name.clone(),
                    integrated_tip: String::new(),
                    default_head: String::new(),
                });
                break;
            };
            let integrated = git
                .resolve_commit(&target.integration_branch)
                .map_err(|err| git_failure(&err))?;
            let moved = tips.get(&target.repository_name) != Some(&integrated);
            let head = if moved {
                String::new()
            } else {
                git.resolve_commit(&default)
                    .map_err(|err| git_failure(&err))?
            };
            facts.push(GitFacts {
                repository: target.repository_name.clone(),
                integrated_tip: integrated,
                default_head: head,
            });
            if moved {
                break;
            }
        }
        Ok(facts)
    }

    /// `_defer`: the stack is rebased onto the moved default branch, the request is kept and the
    /// stack verifies again.
    fn defer(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        heads: &BTreeMap<String, String>,
        mode: &str,
        reason: &str,
        guard: bool,
    ) -> Result<(), WgError> {
        if guard {
            let busy = self.integration_busy_for(stack_id)?;
            guard_defer(&stack_id.to_string(), busy)?;
        }
        let moved = Moved::Deferred {
            mode: mode.to_owned(),
            reason: reason.to_owned(),
        };
        self.0.runtime.decide(
            context,
            |session| stack_world(session, stack_id),
            |world, stamp| move_bases(world, stamp, stack_id, heads, &moved),
        )?;
        self.rebuild_stack(context, stack_id, Some(heads))
    }

    /// `_integration_busy`: integration work for this stack is queued or running.
    pub(crate) fn integration_busy_for(&mut self, stack_id: Id) -> Result<bool, WgError> {
        self.0
            .runtime
            .store
            .read(|session| super::integration_busy(session, stack_id))
    }

    /// `_fast_forward`, checks: every repository is checked before any branch moves.
    fn fast_forward_checks(
        &self,
        base: &LandBase,
        targets: &[IntegrationTarget],
        tips: &BTreeMap<String, String>,
    ) -> Result<Vec<wg_stacks::landing::FfMove>, WgError> {
        let mut checks = Vec::new();
        for target in targets {
            let default = base
                .repos
                .iter()
                .find(|repo| repo.name == target.repository_name)
                .and_then(|repo| repo.default_branch.clone())
                .unwrap_or_default();
            let tip = tips
                .get(&target.repository_name)
                .cloned()
                .unwrap_or_default();
            let source = git_at!(Path::new(&target.source_path));
            let descends = source
                .is_ancestor(&target.base_sha, &tip)
                .map_err(|err| git_failure(&err))?;
            let holder = source
                .worktrees()
                .map_err(|err| git_failure(&err))?
                .into_iter()
                .find(|entry| {
                    entry.branch.as_deref() == Some(default.as_str())
                        && Path::new(&entry.path).exists()
                        && !entry.prunable
                })
                .map(|entry| {
                    let clean = git_at!(Path::new(&entry.path)).is_clean().unwrap_or(false);
                    (entry.path, clean)
                });
            checks.push(FfCheck {
                repository: target.repository_name.clone(),
                default_branch: default,
                base_sha: target.base_sha.clone(),
                tip,
                descends,
                holder,
            });
        }
        fast_forward_plan(&checks)
    }

    /// `_fast_forward`, moves: each default branch moves from its base to the verified tip; a
    /// move that fails undoes the ones already made.
    fn fast_forward(
        &mut self,
        plans: &[wg_stacks::landing::FfMove],
        sources: &BTreeMap<String, String>,
    ) -> Result<(), WgError> {
        let mut moved: Vec<wg_stacks::landing::FfMove> = Vec::new();
        for step in plans {
            let made = self.make_move(step, sources);
            if made.is_err() {
                for undo in rollback_order(&moved) {
                    let _ = self.undo_move(undo, sources);
                }
                return Err(moved_while_landing());
            }
            moved.push(step.clone());
        }
        Ok(())
    }

    /// Where a move runs: the checkout that holds the branch, else the repository's own.
    fn git_for_move(
        &mut self,
        step: &wg_stacks::landing::FfMove,
        sources: &BTreeMap<String, String>,
    ) -> Result<wg_git::Git, WgError> {
        match &step.step {
            FfStep::MergeFfOnly { holder } => Ok(git_at!(holder)),
            FfStep::UpdateRef => sources
                .get(&step.repository)
                .map(|path| git_at!(path))
                .ok_or_else(|| {
                    WgError::workgraph(format!("no source checkout for {}", step.repository))
                }),
        }
    }

    fn make_move(
        &mut self,
        step: &wg_stacks::landing::FfMove,
        sources: &BTreeMap<String, String>,
    ) -> Result<(), WgError> {
        let args = move_args(step);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.git_for_move(step, sources)?
            .run_with(&refs, RunOptions::CHECKED)
            .map(|_| ())
            .map_err(|err| git_failure(&err))
    }

    fn undo_move(
        &mut self,
        step: &wg_stacks::landing::FfMove,
        sources: &BTreeMap<String, String>,
    ) -> Result<(), WgError> {
        let args = rollback_args(step);
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.git_for_move(step, sources)?
            .run_with(&refs, RunOptions::UNCHECKED)
            .map(|_| ())
            .map_err(|err| git_failure(&err))
    }

    /// `_finish`: the acceptances are recorded and the stack is marked landed.
    fn finish_landing_rows(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        run_id: Option<Id>,
        tips: &BTreeMap<String, String>,
        mode: &str,
        reason: &str,
    ) -> Result<Id, WgError> {
        let loaded = self.0.runtime.query(
            |session| {
                let world = stack_world(session, stack_id)?;
                let run = match run_id {
                    Some(id) => session.fetch::<VerificationRun>(&RowKey::VerificationRun(id))?,
                    None => None,
                };
                let manifest: Option<RevisionManifest> =
                    match run.as_ref().and_then(|row| row.manifest_id) {
                        Some(id) => session.manifest_by_id(id)?,
                        None => None,
                    };
                let mut attempts = BTreeMap::new();
                for task in world.tasks.tasks.keys() {
                    let succeeded: Vec<AgentAttempt> =
                        session.attempts_of_task(*task, &[AttemptStatus::Succeeded])?;
                    if let Some(latest) = succeeded.iter().max_by_key(|row| row.number) {
                        attempts.insert(*task, latest.id);
                    }
                }
                Ok((
                    world,
                    AcceptFacts {
                        run,
                        manifest,
                        attempts,
                    },
                ))
            },
            |loaded| Ok(loaded.clone()),
        )?;
        let (_, facts) = loaded;
        Ok(self
            .0
            .runtime
            .decide_then(
                context,
                |session| stack_world(session, stack_id),
                |world, stamp| finish_landing(world, stamp, stack_id, tips, mode, reason, &facts),
                index_written,
            )?
            .reply)
    }

    /// `_remove_integration`: the integration worktrees and branches go after the landing.
    fn remove_integration(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        targets: &[IntegrationTarget],
    ) -> Result<(), WgError> {
        for target in targets {
            let path = integration_path(
                &native_root(self.0.paths.worktrees_dir().as_str()),
                stack_id,
                &target.repository_name,
            );
            let text = path.to_string_lossy().into_owned();
            let active = self
                .0
                .runtime
                .store
                .read(|session| session.worktree_by_path(&text))?
                .filter(|row| row.status == WorktreeStatus::Active);
            if let Some(row) = active {
                self.remove_worktree(context, row.id)?;
            }
            git_at!(Path::new(&target.source_path))
                .branch_delete(&target.integration_branch, RunOptions::UNCHECKED)
                .map_err(|err| git_failure(&err))?;
            self.0.runtime.decide(
                context,
                |session| stack_world(session, stack_id),
                |world, stamp| {
                    integration_removed(
                        world,
                        stamp,
                        stack_id,
                        &target.repository_name,
                        &target.integration_branch,
                    )
                },
            )?;
        }
        Ok(())
    }
}
