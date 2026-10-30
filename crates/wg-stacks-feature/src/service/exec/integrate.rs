//! `StackIntegrator.integrate`: the `stack_integrate` job. It merges every due ready item of one
//! repository, in order, into the stack's integration branch.

use std::collections::BTreeMap;

use crate::spec::exec::integrate::{
    Done, begin_item, due_item, finish_item, inactive_result, integrate_followup, ran_result,
    settled,
};
use crate::spec::exec::verify::verification_job;
use crate::spec::exec::worktree::{IntegrationTarget, integration_target};
use serde_json::Value;
use wg_git::Git;
use wg_rows::task_world_sql::TaskWorld;
use wg_runtimespec::RequestContext;
use wg_stacks::merge::ItemStep;
use wg_store::Session;
use wg_types::{Id, WgError};

use super::stack_world;
use crate::spec::native_root;

use crate::service::{jobs_under, latest_event_id};
use wg_runtime::index_written;

/// The head the task left in each stack repository it changed (`StackIntegrator.heads`).
fn heads_of(
    session: &mut Session<'_>,
    world: &TaskWorld,
    stack_id: Id,
    item_id: Id,
) -> Result<BTreeMap<Id, String>, WgError> {
    let task_id = world
        .stacks
        .items
        .get(&item_id)
        .map(|item| item.task_id)
        .ok_or_else(|| WgError::workgraph(format!("stack item {item_id} vanished")))?;
    world
        .stacks
        .repositories_of(stack_id)
        .try_fold(BTreeMap::new(), |heads, link| {
            let recorded = session.latest_recorded_head(task_id, link.repository_id)?;
            Ok(match recorded {
                Some(found) => heads
                    .into_iter()
                    .chain(std::iter::once((link.repository_id, found.head_sha)))
                    .collect(),
                None => heads,
            })
        })
}

use super::Exec;

impl Exec<'_> {
    /// `StackIntegrator.integrate`.
    pub fn stack_integrate_job(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        repository_id: Id,
    ) -> Result<Value, WgError> {
        let (world, checkouts) = self.0.runtime.query(
            |session| {
                let missing = || {
                    WgError::not_found(format!(
                        "stack {stack_id} has no repository {repository_id}"
                    ))
                };
                if session.stack_by_id(stack_id)?.is_none() {
                    return Err(missing());
                }
                let world = stack_world(session, stack_id)?;
                Ok((world, session.checkouts(repository_id)?))
            },
            |loaded| Ok(loaded.clone()),
        )?;
        let Some(target) = integration_target(&world, stack_id, repository_id, &checkouts)? else {
            return Ok(inactive_result());
        };
        let path = self.ensure_worktree(context, &target)?;
        let git = git_at!(&path);
        let Merged {
            merged,
            skipped,
            conflicts,
            progressed,
        } = self.merge_due_items(context, &target, &git)?;
        if progressed {
            self.followup_integration(context, stack_id, repository_id)?;
        }
        self.enqueue_if_settled(context, stack_id)?;
        Ok(ran_result(
            &target.repository_name,
            &merged,
            &skipped,
            &conflicts,
        ))
    }

    /// The loop of `integrate`: the due items, one at a time. Returns the task keys merged,
    /// skipped and conflicting, and whether anything merged.
    fn merge_due_items(
        &mut self,
        context: &RequestContext,
        target: &IntegrationTarget,
        git: &Git,
    ) -> Result<Merged, WgError> {
        let stack_id = target.stack_id;
        let mut processed: Vec<Id> = Vec::new();
        let mut merged = Vec::new();
        let mut skipped = Vec::new();
        let mut conflicts = Vec::new();
        loop {
            let due = self.0.runtime.query(
                |session| stack_world(session, stack_id),
                |world| due_item(world, stack_id, &processed),
            )?;
            let Some(item_id) = due else {
                break;
            };
            processed.push(item_id);
            let (kind, key) = self.integrate_item(context, target, git, item_id)?;
            match kind {
                ItemKind::Conflict => conflicts.push(key),
                ItemKind::Skipped => skipped.push(key),
                ItemKind::Merged => merged.push(key),
                ItemKind::Noop => {}
            }
        }
        let progressed = !merged.is_empty();
        Ok(Merged {
            merged,
            skipped,
            conflicts,
            progressed,
        })
    }

    /// `_integrate_item`: one item, in two transactions around the merge.
    fn integrate_item(
        &mut self,
        context: &RequestContext,
        target: &IntegrationTarget,
        git: &Git,
        item_id: Id,
    ) -> Result<(ItemKind, String), WgError> {
        let stack_id = target.stack_id;
        let begun = self
            .0
            .runtime
            .decide_then(
                context,
                |session| {
                    let world = stack_world(session, stack_id)?;
                    let heads = heads_of(session, &world, stack_id, item_id)?;
                    Ok((world, heads))
                },
                |(world, heads), stamp| begin_item(world, stamp, item_id, target, heads),
                index_written,
            )?
            .reply;
        let head = match &begun.step {
            ItemStep::MergeUnchanged => return Ok((ItemKind::Merged, begun.task_key)),
            ItemStep::Skip => return Ok((ItemKind::Skipped, begun.task_key)),
            ItemStep::Noop => return Ok((ItemKind::Noop, begun.task_key)),
            ItemStep::Merge(head) => head.clone(),
        };
        let outcome = self.merge_head(git, &head, &begun.message)?;
        let done = self
            .0
            .runtime
            .decide_then(
                context,
                |session| {
                    let world = stack_world(session, stack_id)?;
                    let heads = heads_of(session, &world, stack_id, item_id)?;
                    Ok((world, heads))
                },
                |(world, heads), stamp| {
                    finish_item(world, stamp, item_id, target, &head, &outcome, heads)
                },
                index_written,
            )?
            .reply;
        let kind = match done {
            Done::Merged => ItemKind::Merged,
            Done::Conflict => ItemKind::Conflict,
        };
        Ok((kind, begun.task_key))
    }

    /// After a merge: the jobs of the stack's other repositories, which it can unblock.
    fn followup_integration(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        exclude: Id,
    ) -> Result<(), WgError> {
        self.0.runtime.decide(
            context,
            |session| {
                let world = stack_world(session, stack_id)?;
                let version = latest_event_id(session, stack_id)?;
                let keys: Vec<String> = world
                    .stacks
                    .repositories_of(stack_id)
                    .map(|link| {
                        crate::spec::exec::integrate::integration_key(
                            stack_id,
                            link.repository_id,
                            version,
                        )
                    })
                    .collect();
                let queue = jobs_under(session, &keys)?;
                Ok((world, version, queue))
            },
            |(world, version, queue), stamp| {
                integrate_followup(world, stamp, stack_id, exclude, *version, queue)
            },
        )?;
        Ok(())
    }

    /// `StackVerifier.enqueue_if_settled`: queues verification once nothing is pending or ready.
    pub(crate) fn enqueue_if_settled(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
    ) -> Result<Option<Id>, WgError> {
        let ready = self.0.runtime.query(
            |session| stack_world(session, stack_id),
            |world| Ok(settled(world, stack_id)),
        )?;
        if ready {
            self.enqueue_verification(context, stack_id, None)
        } else {
            Ok(None)
        }
    }

    /// `StackVerifier._enqueue`: one `stack_verify` job keyed by the integration tips.
    pub(crate) fn enqueue_verification(
        &mut self,
        context: &RequestContext,
        stack_id: Id,
        version: Option<Id>,
    ) -> Result<Option<Id>, WgError> {
        let worktrees = native_root(self.0.paths.worktrees_dir().as_str());
        let repos = self.0.runtime.query(
            |session| stack_world(session, stack_id),
            |world| Ok(crate::spec::exec::repos_of(world, stack_id)),
        )?;
        let tips: Vec<(String, String)> = repos
            .iter()
            .map(|repo| {
                let path =
                    crate::spec::exec::worktree::integration_path(&worktrees, stack_id, &repo.name);
                let tip = path
                    .exists()
                    .then(|| git_at!(&path).head().ok().flatten())
                    .flatten();
                (
                    repo.name.clone(),
                    wg_stacks::verify::tip_or_base(tip.as_deref(), &repo.base_sha),
                )
            })
            .collect();
        Ok(self
            .0
            .runtime
            .decide(
                context,
                |session| {
                    let world = stack_world(session, stack_id)?;
                    let latest = latest_event_id(session, stack_id)?;
                    let key = crate::spec::exec::verify::verification_key(stack_id, &tips, version);
                    let queue = jobs_under(session, &[key])?;
                    Ok((world, latest, queue))
                },
                |(world, latest, queue), stamp| {
                    Ok(verification_job(
                        world,
                        stamp,
                        stack_id,
                        &tips,
                        version,
                        version.unwrap_or(*latest),
                        queue,
                    ))
                },
            )?
            .reply)
    }
}

/// The task keys a pass over the due items merged, skipped and found conflicting.
struct Merged {
    merged: Vec<String>,
    skipped: Vec<String>,
    conflicts: Vec<String>,
    progressed: bool,
}

/// How `integrate_item` ended for one item.
enum ItemKind {
    Merged,
    Skipped,
    Conflict,
    Noop,
}
