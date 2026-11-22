//! The land-review guardrail of `git sync`: a task whose `Task-ID` trailer commit is reachable
//! from the repository's integration branch moves to `awaiting_review`, with a comment naming the
//! commit and the branch. The decisions are in [`crate::spec::land_review`].
//!
//! Git is read outside any transaction. The decision then reloads the tasks and their comments
//! in its own transaction, so a task another writer already moved is left alone, and it applies
//! the status change through the ordinary task transition, which records the history event.
//! Idempotency: a moved task is no longer in an eligible status, and the comment's marker keeps
//! a task that was reopened afterwards from being moved again by the same commit.

use std::collections::BTreeSet;

use wg_git::Git;
use wg_model::Decision;
use wg_runtime::App;
use wg_runtime::spec::RequestContext;
use wg_tasks::lifecycle::LAND_REVIEW_SOURCES;
use wg_tasks::transition::{Change, transition};
use wg_types::{CommentKind, Id, TaskStatus, WgError};

use super::store;
use super::sync::{NO_CHECK, RepoState};
use crate::spec::git_error;
use crate::spec::land_review as plan;

/// Moves the tasks whose trailer commits the integration branch holds. Returns how many moved.
pub(crate) fn land_review(
    app: &mut App,
    context: &RequestContext,
    git: &Git,
    state: &RepoState,
) -> Result<i64, WgError> {
    let repository_id = state.repository.id;
    let configured = wg_registry::admin::integration_branch(&state.repository);
    let statuses = plan::eligible_statuses();
    let (enabled, branch, landings) = app.runtime.store.read(|session| {
        let workspace = session
            .workspace_by_id(state.workspace_id)?
            .ok_or_else(|| WgError::not_found("workspace not found"))?;
        if !wg_registry::settings::review_on_land(&workspace.settings) {
            return Ok((false, None, Vec::new()));
        }
        let from_stack = match configured {
            Some(_) => None,
            None => store::latest_stack_integration_branch(session, repository_id)?,
        };
        let Some(branch) = plan::integration_branch(configured, from_stack) else {
            return Ok((true, None, Vec::new()));
        };
        let landings = store::open_trailer_landings(session, repository_id, &statuses)?;
        Ok((true, Some(branch), landings))
    })?;
    let (true, Some(branch)) = (enabled, branch) else {
        return Ok(0);
    };
    if landings.is_empty() {
        return Ok(0);
    }
    let reference = plan::branch_ref(&branch);
    let output = git
        .run_with(&["rev-list", reference.as_str()], NO_CHECK)
        .map_err(git_error)?;
    if output.returncode != 0 {
        return Ok(0);
    }
    let reachable: BTreeSet<String> = output
        .stdout
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let landed = plan::landed(&landings, &reachable);
    if landed.is_empty() {
        return Ok(0);
    }
    let task_ids: Vec<Id> = landed.keys().copied().collect();
    let workspace_id = state.workspace_id;
    app.runtime
        .decide(
            context,
            |session| session.task_world(Some(workspace_id), &task_ids),
            |world, stamp| {
                let mut tasks = world.tasks.clone();
                let mut ids = stamp.ids();
                let mut decision = Decision::new(());
                let mut moved: i64 = 0;
                for (task_id, sha) in &landed {
                    let Some(task) = tasks.task(*task_id) else {
                        continue;
                    };
                    let workspace = task.workspace_id;
                    if !LAND_REVIEW_SOURCES.contains(&task.status)
                        || tasks
                            .comments_of(*task_id)
                            .any(|comment| plan::is_marked(&comment.data, sha))
                    {
                        continue;
                    }
                    let why = plan::reason(sha, &branch);
                    let applied = transition(
                        &tasks,
                        &world.stacks,
                        stamp,
                        ids,
                        *task_id,
                        TaskStatus::AwaitingReview,
                        Change {
                            reason: Some(&why),
                            landed: true,
                            ..Change::default()
                        },
                    )?;
                    let made = wg_tasks::accept::comment_step(
                        stamp,
                        applied.ids,
                        *task_id,
                        workspace,
                        CommentKind::Comment,
                        &plan::comment_body(sha, &branch),
                        &plan::comment_data(sha, &branch),
                        None,
                        None,
                    );
                    tasks = applied.tasks.apply_all(made.decision.writes.iter());
                    ids = made.ids;
                    decision = decision.then(applied.decision).then(made.decision);
                    moved += 1;
                }
                Ok(decision.with_reply(moved))
            },
        )
        .map(|outcome| outcome.reply)
}
