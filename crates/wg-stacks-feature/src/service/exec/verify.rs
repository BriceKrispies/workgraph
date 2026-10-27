//! `StackVerifier`: the `stack_verify` job, bisection, ejection and the rebuild of the
//! integration branches.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::spec::exec::verify::{
    EjectPlan, Plan, ReplayedMerge, Start, bisect_probe, eject_bisected, eject_manual, eject_plan,
    failed_on_base, manifest_entries, merge_sequence, probe_context, rebuild_apply, tip_context,
    verification_errored, verification_failed, verification_passed, verification_started,
    verify_start,
};
use crate::spec::exec::worktree::{IntegrationTarget, integration_target};
use serde_json::{Value, json};
use wg_git::RunOptions;
use wg_model::entity::VerificationRun;
use wg_runtimespec::RequestContext;
use wg_stacks::bisect::{
    BisectEntry, Bisection, Entry, ProbeVerdict, bisect_entry, prefix_shas, probe_failed_checks,
    probe_verdict,
};
use wg_stacks::rebuild::{RebuildTarget, rebuild_plan};
use wg_stacks::regression::{BaseOutcomes, RunCheck, RunResult, errored_checks, regressions};
use wg_stacks::verify::{
    BaseVerdict, CACHE_SCAN_LIMIT, CachedRun, Errored, REASON_FULL_PASSED, REASON_NO_CHECKS,
    REASON_NO_NEW_FAILURES, StackRepoBase, Stage, TipStep, baselines, bases, cached_outcomes,
    error_tails, in_scope, is_cancellable_verify_job, judge_against_base, judge_tip,
    missing_base_checks, pending_checks, with_base_run,
};
use wg_types::{ErrorCode, Id, VerificationStatus, WgError, parse_id};
use wg_verify::{ApplicableCheck, CheckSpec};
use wg_verify_feature::service::reads::{verification_check_results, verification_runs};
use wg_verify_feature::spec::sql::{Page, RunFilter};
use wg_wire::convention::{dict, text};

use super::stack_world;

use crate::service::load_world;
use crate::spec::git_failure;
use wg_runtime::index_written;

/// The checkouts of a stack's integration worktrees, with their targets, in repository order.
struct Worktrees {
    targets: Vec<IntegrationTarget>,
    paths: BTreeMap<String, PathBuf>,
    tips: BTreeMap<String, String>,
}

/// What a search over prefixes ended with.
enum Searched {
    Cancelled,
    Errored(Errored),
    Found(usize, RunResult),
}

use super::Exec;

impl Exec<'_> {
    /// `StackIntegrator.target` for one repository of a stack.
    pub(crate) fn stack_target(
        &mut self,
        stack_id: Id,
        repository_id: Id,
    ) -> Result<Option<IntegrationTarget>, WgError> {
        self.0.runtime.query(
            |session| {
                let world = stack_world(session, stack_id)?;
                Ok((world, session.checkouts(repository_id)?))
            },
            |(world, checkouts)| integration_target(world, stack_id, repository_id, checkouts),
        )
    }

    /// `StackVerifier.verify`: verifies the integration tip once the stack has settled; bisects
    /// and ejects on failure.
    pub fn stack_verify_job(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Value, WgError> {
        let started = self.0.runtime.query(
            |session| {
                let world = stack_world(session, stack_id)?;
                let configs = match world.stacks.stack(stack_id) {
                    Some(stack) => session.verification_configs(stack.workspace_id)?,
                    None => Vec::new(),
                };
                Ok((world, configs))
            },
            |(world, configs)| verify_start(world, configs, stack_id),
        )?;
        let plan = match started {
            Start::Inactive => return Ok(json!({"status": "inactive"})),
            Start::Waiting => return Ok(json!({"status": "waiting"})),
            Start::Go(plan) => *plan,
        };
        if self.rebase_if_stale(context, stack_id)? {
            return Ok(json!({"status": "rebased"}));
        }
        let Some(tree) = self.integration_worktrees(context, stack_id, &plan.repos)? else {
            return Ok(json!({"status": "inactive"}));
        };
        let names: Vec<String> = plan
            .checks
            .iter()
            .map(|check| check.spec.name.clone())
            .collect();
        self.0.runtime.decide(
            context,
            |session| stack_world(session, stack_id),
            |world, stamp| verification_started(world, stamp, stack_id, &tree.tips, &names),
        )?;
        if plan.checks.is_empty() {
            return self.verified(context, stack_id, None, &tree.tips, REASON_NO_CHECKS, None);
        }
        let keep = |check: &ApplicableCheck| in_scope(&plan.repos, &check.spec);
        let checkouts: BTreeMap<String, String> = tree
            .paths
            .iter()
            .map(|(name, path)| (name.clone(), path.to_string_lossy().into_owned()))
            .collect();
        let entries = manifest_entries(&tree.targets, &checkouts, &tree.tips, true);
        let run = self.run_stack_verification(
            context,
            stack_id,
            plan.parent_id,
            entries,
            &keep,
            tip_context(stack_id, &tree.tips),
            cancel,
        )?;
        for path in tree.paths.values() {
            git_at!(path)
                .clean_force(RunOptions::UNCHECKED)
                .map_err(|err| git_failure(&err))?;
        }
        if cancel() {
            return Ok(json!({"status": "cancelled", "run_id": run.id}));
        }
        match judge_tip(&run) {
            TipStep::Errored(errored) => self.errored(context, stack_id, &errored),
            TipStep::Passed => self.verified(
                context,
                stack_id,
                Some(&run.id),
                &tree.tips,
                REASON_FULL_PASSED,
                None,
            ),
            TipStep::MeasureBase(failed) => {
                let specs: BTreeMap<String, CheckSpec> = plan
                    .checks
                    .iter()
                    .map(|check| (check.spec.name.clone(), check.spec.clone()))
                    .collect();
                let base = self.base_outcomes(context, stack_id, &plan, &tree, &failed, &specs)?;
                let marks = baselines(&specs.values().cloned().collect::<Vec<_>>());
                match judge_against_base(&run, &base, &marks) {
                    BaseVerdict::Errored(errored) => self.errored(context, stack_id, &errored),
                    BaseVerdict::Tolerated(found) => self.verified(
                        context,
                        stack_id,
                        Some(&run.id),
                        &tree.tips,
                        REASON_NO_NEW_FAILURES,
                        Some(&found),
                    ),
                    BaseVerdict::Regressed(_) => {
                        self.bisect(context, stack_id, &plan, &tree, &run, &base, &specs, cancel)
                    }
                }
            }
        }
    }

    /// The integration worktree and tip of every repository; `None` when the stack is no longer
    /// active.
    fn integration_worktrees(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        repos: &[StackRepoBase],
    ) -> Result<Option<Worktrees>, WgError> {
        let mut tree = Worktrees {
            targets: Vec::new(),
            paths: BTreeMap::new(),
            tips: BTreeMap::new(),
        };
        for repo in repos {
            let repository_id = parse_id(&repo.repository_id)?;
            let Some(target) = self.stack_target(stack_id, repository_id)? else {
                return Ok(None);
            };
            let path = self.ensure_worktree(context, &target)?;
            let tip = git_at!(&path)
                .head()
                .map_err(|err| git_failure(&err))?
                .ok_or_else(|| {
                    WgError::new(
                        ErrorCode::GitError,
                        format!("integration worktree {} has no HEAD", path.display()),
                    )
                })?;
            tree.paths.insert(repo.name.clone(), path);
            tree.tips.insert(repo.name.clone(), tip);
            tree.targets.push(target);
        }
        Ok(Some(tree))
    }

    /// `_verification_errored`.
    fn errored(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        errored: &Errored,
    ) -> Result<Value, WgError> {
        self.0.runtime.decide(
            context,
            |session| stack_world(session, stack_id),
            |world, stamp| verification_errored(world, stamp, stack_id, errored),
        )?;
        Ok(json!({
            "status": "errored",
            "run_id": errored.run_id,
            "checks": errored.checks(),
            "stage": errored.stage.as_str(),
        }))
    }

    /// `_passed`, then the landing the verification may have asked for.
    fn verified(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        run_id: Option<&str>,
        tips: &BTreeMap<String, String>,
        reason: &str,
        tolerated: Option<&[(String, Vec<String>)]>,
    ) -> Result<Value, WgError> {
        let verified = self
            .0
            .runtime
            .decide_then(
                context,
                |session| stack_world(session, stack_id),
                |world, stamp| {
                    verification_passed(world, stamp, stack_id, run_id, tips, reason, tolerated)
                },
                index_written,
            )?
            .reply;
        let landing = self.after_verified(context, stack_id)?;
        Ok(json!({
            "status": "verified",
            "run_id": run_id,
            "verified": verified,
            "landing": landing,
        }))
    }

    // ---------------------------------------------------------------- base outcomes

    /// `_base_outcomes`: how each failed check finished on the stack's base, from an earlier run
    /// at the base or from a new one.
    fn base_outcomes(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        plan: &Plan,
        tree: &Worktrees,
        failed: &[(String, Option<String>)],
        specs: &BTreeMap<String, CheckSpec>,
    ) -> Result<BaseOutcomes, WgError> {
        if failed.is_empty() {
            return Ok(BaseOutcomes::new());
        }
        let cached = self.cached_base_outcomes(plan, failed, specs)?;
        let missing = missing_base_checks(failed, &cached);
        if missing.is_empty() {
            return Ok(cached);
        }
        let at_bases = bases(&plan.repos);
        let names: BTreeSet<String> = missing.iter().cloned().collect();
        let run = self.with_bisect_worktrees(context, &tree.targets, |app, bisect| {
            app.probe(
                context,
                stack_id,
                plan,
                &tree.targets,
                bisect,
                &at_bases,
                &names,
                0,
                &|| false,
            )
        })?;
        Ok(with_base_run(
            cached,
            &run.id,
            &run.checks,
            &missing,
            failed,
        ))
    }

    /// `_cached_outcomes`: outcomes from the newest runs of the workspace whose manifest is at
    /// the bases.
    fn cached_base_outcomes(
        &mut self,
        plan: &Plan,
        failed: &[(String, Option<String>)],
        specs: &BTreeMap<String, CheckSpec>,
    ) -> Result<BaseOutcomes, WgError> {
        let workspace = plan.workspace_id;
        let limit = i64::try_from(CACHE_SCAN_LIMIT).unwrap_or(i64::MAX);
        let runs: Vec<VerificationRun> = self.0.runtime.store.read(|session| {
            verification_runs(
                session,
                workspace,
                &RunFilter::default(),
                Page { limit, offset: 0 },
            )
        })?;
        let mut found = BaseOutcomes::new();
        for run in &runs {
            if pending_checks(failed, &found, specs).is_empty() {
                break;
            }
            let cached = self.cached_run(run)?;
            let got = cached_outcomes(&[cached], &plan.repos, failed, specs);
            found = got.iter().fold(found, |found, (name, outcome)| {
                if found.contains(name) {
                    found
                } else {
                    found.with(name.to_owned(), outcome.clone())
                }
            });
        }
        Ok(found)
    }

    /// A run of the workspace as the base cache reads it.
    fn cached_run(&mut self, run: &VerificationRun) -> Result<CachedRun, WgError> {
        let (manifest, results) = self.0.runtime.store.read(|session| {
            let manifest = match run.manifest_id {
                Some(id) => session.manifest_by_id(id)?,
                None => None,
            };
            Ok((manifest, verification_check_results(session, run.id)?))
        })?;
        Ok(CachedRun {
            id: run.id.to_string(),
            manifest_entries: manifest.map(|row| {
                row.entries
                    .iter()
                    .cloned()
                    .map(Value::Object)
                    .collect::<Vec<Value>>()
            }),
            config_snapshot: run
                .config_snapshot
                .iter()
                .cloned()
                .map(Value::Object)
                .collect(),
            checks: results
                .iter()
                .map(|row| RunCheck {
                    name: row.name.clone(),
                    required: row.required,
                    status: row
                        .status
                        .parse::<VerificationStatus>()
                        .unwrap_or(VerificationStatus::Error),
                    repository_id: row.repository_id.map(|id| id.to_string()),
                    failure_signatures: row.failure_signatures.iter().cloned().collect(),
                    output_excerpt: row.output_excerpt.clone(),
                })
                .collect(),
        })
    }

    // ---------------------------------------------------------------- bisect

    /// `_bisect`: finds the first item whose prefix fails, ejects it, rebuilds without it and
    /// queues the verification of the rebuilt tip.
    #[allow(clippy::too_many_arguments)]
    fn bisect(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        plan: &Plan,
        tree: &Worktrees,
        tip_run: &RunResult,
        base: &BaseOutcomes,
        specs: &BTreeMap<String, CheckSpec>,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Value, WgError> {
        let marks = baselines(&specs.values().cloned().collect::<Vec<_>>());
        let failing = regressions(&tip_run.checks, Some(base), &marks);
        let sequence: Vec<Entry> = self
            .0
            .runtime
            .decide(
                context,
                |session| stack_world(session, stack_id),
                |world, stamp| {
                    verification_failed(world, stamp, stack_id, &tip_run.id, &tree.tips, &failing)
                },
            )?
            .reply;
        if let BisectEntry::FailedOnBase {
            reason,
            failed_checks,
            run_id,
        } = bisect_entry(sequence.len(), &tip_run.id, &tip_run.checks, base, &marks)
        {
            self.0.runtime.decide(
                context,
                |session| stack_world(session, stack_id),
                |world, stamp| {
                    failed_on_base(
                        world,
                        stamp,
                        stack_id,
                        run_id.as_deref(),
                        &failed_checks,
                        reason,
                    )
                },
            )?;
            return Ok(json!({"status": "failed_on_base", "run_id": run_id}));
        }
        let at_bases = bases(&plan.repos);
        let names: BTreeSet<String> = failing.iter().map(|(name, _)| name.clone()).collect();
        let searched = self.with_bisect_worktrees(context, &tree.targets, |app, bisect| {
            let mut search = Bisection::new(sequence.len(), tip_run.clone());
            while let Some(middle) = search.next_probe() {
                if cancel() {
                    return Ok(Searched::Cancelled);
                }
                let shas = prefix_shas(&at_bases, &sequence, middle)?;
                let run = app.probe(
                    context,
                    stack_id,
                    plan,
                    &tree.targets,
                    bisect,
                    &shas,
                    &names,
                    middle,
                    cancel,
                )?;
                if cancel() {
                    return Ok(Searched::Cancelled);
                }
                let verdict = probe_verdict(&run.checks, base, &marks);
                if matches!(verdict, ProbeVerdict::Errored(_)) {
                    return Ok(Searched::Errored(Errored {
                        stage: Stage::Bisect,
                        run_id: Some(run.id.clone()),
                        errors: error_tails(&errored_checks(&run.checks)),
                    }));
                }
                let passed = verdict == ProbeVerdict::Passed;
                let failed_checks = probe_failed_checks(&run.checks);
                app.0.runtime.decide(
                    context,
                    |session| stack_world(session, stack_id),
                    |world, stamp| {
                        bisect_probe(
                            world,
                            stamp,
                            stack_id,
                            middle,
                            &shas,
                            passed,
                            &run.id,
                            &failed_checks,
                        )
                    },
                )?;
                search = search.record(middle, passed, run);
            }
            let (position, run) = search.finish();
            Ok(Searched::Found(position, run))
        })?;
        let (position, high_run) = match searched {
            Searched::Cancelled => return Ok(json!({"status": "cancelled"})),
            Searched::Errored(errored) => return self.errored(context, stack_id, &errored),
            Searched::Found(position, run) => (position, run),
        };
        if cancel() {
            return Ok(json!({"status": "cancelled"}));
        }
        let culprit = sequence
            .get(position.saturating_sub(1))
            .cloned()
            .ok_or_else(|| WgError::new(ErrorCode::WorkgraphError, "bisection found no culprit"))?;
        let signatures = regressions(&high_run.checks, Some(base), &marks);
        let task_key = self
            .0
            .runtime
            .decide_then(
                context,
                |session| stack_world(session, stack_id),
                |world, stamp| {
                    eject_bisected(
                        world,
                        stamp,
                        stack_id,
                        &culprit,
                        position,
                        &high_run.id,
                        &signatures,
                    )
                },
                index_written,
            )?
            .reply;
        self.rebuild_branches(context, stack_id, &tree.targets, &plan.repos)?;
        self.enqueue_verification(context, stack_id, None)?;
        Ok(json!({
            "status": "ejected",
            "task": task_key,
            "position": position,
            "run_id": high_run.id,
        }))
    }

    /// `_probe`: one revision is checked out in the bisect worktrees and the named full checks
    /// run there.
    #[allow(clippy::too_many_arguments)]
    fn probe(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        plan: &Plan,
        targets: &[IntegrationTarget],
        bisect: &BTreeMap<String, (Id, PathBuf)>,
        shas: &BTreeMap<String, String>,
        names: &BTreeSet<String>,
        position: usize,
        cancel: &dyn Fn() -> bool,
    ) -> Result<RunResult, WgError> {
        for (name, sha) in shas {
            let Some((_, path)) = bisect.get(name) else {
                continue;
            };
            let git = git_at!(path);
            git.checkout_detach_force(sha, RunOptions::CHECKED)
                .map_err(|err| git_failure(&err))?;
            git.clean_force(RunOptions::CHECKED)
                .map_err(|err| git_failure(&err))?;
        }
        let checkouts: BTreeMap<String, String> = bisect
            .iter()
            .map(|(name, (_, path))| (name.clone(), path.to_string_lossy().into_owned()))
            .collect();
        let keep = |check: &ApplicableCheck| {
            in_scope(&plan.repos, &check.spec) && names.contains(&check.spec.name)
        };
        self.run_stack_verification(
            context,
            stack_id,
            plan.parent_id,
            manifest_entries(targets, &checkouts, shas, false),
            &keep,
            probe_context(stack_id, position, shas),
            cancel,
        )
    }

    /// `_worktrees`: bisect worktrees for every target, removed when the body ends.
    fn with_bisect_worktrees<T>(
        &mut self,
        context: &RequestContext,
        targets: &[IntegrationTarget],
        body: impl FnOnce(&mut Exec<'_>, &BTreeMap<String, (Id, PathBuf)>) -> Result<T, WgError>,
    ) -> Result<T, WgError> {
        let mut made: BTreeMap<String, (Id, PathBuf)> = BTreeMap::new();
        let mut failure: Option<WgError> = None;
        for target in targets {
            match self.bisect_worktree(context, target) {
                Ok(found) => {
                    made.insert(target.repository_name.clone(), found);
                }
                Err(err) => {
                    failure = Some(err);
                    break;
                }
            }
        }
        let outcome = match failure {
            Some(err) => Err(err),
            None => body(self, &made),
        };
        let removed = made
            .values()
            .try_for_each(|(row_id, _)| self.remove_worktree(context, *row_id));
        let value = outcome?;
        removed?;
        Ok(value)
    }

    // ---------------------------------------------------------------- rebuild

    /// `StackVerifier.rebuild`: each integration branch is reset to its base and the items still
    /// on it are replayed, in merge order.
    pub(crate) fn rebuild_branches(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        targets: &[IntegrationTarget],
        repos: &[StackRepoBase],
    ) -> Result<(), WgError> {
        let sequence = self.0.runtime.query(
            |session| stack_world(session, stack_id),
            |world| Ok(merge_sequence(world, stack_id)),
        )?;
        let at_bases = bases(repos);
        let rebuild_targets: Vec<RebuildTarget> = targets
            .iter()
            .map(|target| RebuildTarget {
                repository_name: target.repository_name.clone(),
                integration_branch: target.integration_branch.clone(),
            })
            .collect();
        let plan = rebuild_plan(&sequence, &rebuild_targets, &at_bases)?;
        let mut replayed: Vec<ReplayedMerge> = Vec::new();
        let mut tips: BTreeMap<String, String> = BTreeMap::new();
        for repo in &plan {
            let Some(target) = targets
                .iter()
                .find(|target| target.repository_name == repo.repository_name)
            else {
                continue;
            };
            let path = self.ensure_worktree(context, target)?;
            let git = git_at!(&path);
            git.clean_force(RunOptions::CHECKED)
                .map_err(|err| git_failure(&err))?;
            git.reset_hard(&repo.base_sha, RunOptions::CHECKED)
                .map_err(|err| git_failure(&err))?;
            for merge in &repo.merges {
                let outcome = self.merge_head(&git, &merge.head, &merge.message)?;
                replayed.push(ReplayedMerge {
                    item_id: parse_id(&merge.item_id)?,
                    repository: repo.repository_name.clone(),
                    head: merge.head.clone(),
                    outcome,
                });
            }
            let tip = git
                .head()
                .map_err(|err| git_failure(&err))?
                .unwrap_or_default();
            tips.insert(repo.repository_name.clone(), tip);
        }
        let branches: BTreeMap<String, String> = targets
            .iter()
            .map(|target| {
                (
                    target.repository_name.clone(),
                    target.integration_branch.clone(),
                )
            })
            .collect();
        self.0.runtime.decide_then(
            context,
            |session| stack_world(session, stack_id),
            |world, stamp| {
                rebuild_apply(
                    world, stamp, stack_id, &branches, &at_bases, &tips, &replayed,
                )
            },
            index_written,
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------- ejection

    /// `StackVerifier.eject`: ejects a merged child an operator has identified as the stack's
    /// failure, then rebuilds without it.
    pub fn stack_eject(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        parent: &str,
        task: &str,
        reason: &str,
    ) -> Result<wg_json::Value, WgError> {
        let scope = self.0.workspace_or_default(workspace)?;
        let stamp = self.0.runtime.stamps.stamp(context)?;
        let plan: EjectPlan = self.0.runtime.query(
            |session| load_world(session, scope),
            |world| eject_plan(world, &stamp, scope, parent, task, reason),
        )?;
        let stack_id = plan.stack_id;
        let task_key = self
            .0
            .runtime
            .decide_then(
                context,
                |session| stack_world(session, stack_id),
                |world, stamp| eject_manual(world, stamp, &plan, reason),
                index_written,
            )?
            .reply;
        self.cancel_verify_jobs(context, stack_id)?;
        self.rebuild_without(context, stack_id)?;
        Ok(dict(vec![
            ("status", text("ejected")),
            ("task", text(&task_key)),
            ("stack_id", text(&stack_id.to_string())),
        ]))
    }

    /// `_cancel_verify_jobs`: the stack's queued `stack_verify` jobs are cancelled and a running
    /// one is asked to stop at its next check boundary.
    fn cancel_verify_jobs(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
    ) -> Result<Vec<Id>, WgError> {
        let active = self
            .0
            .runtime
            .store
            .read(|session| session.active_slice("stack_verify"))?;
        let wanted = stack_id.to_string();
        let ids: Vec<Id> = active
            .jobs
            .values()
            .filter(|job| {
                is_cancellable_verify_job(
                    &job.kind,
                    job.status.as_str(),
                    &Value::Object(job.payload.clone()),
                    &wanted,
                )
            })
            .map(|job| job.id)
            .collect();
        for job_id in &ids {
            self.0.runtime.decide(
                context,
                |session| session.job_slice(*job_id),
                |slice, stamp| wg_jobs::queue::cancel(slice, *job_id, stamp),
            )?;
        }
        Ok(ids)
    }
}
