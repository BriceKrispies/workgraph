//! Shell: `WorktreeManager.capture_outcome`: looks at each checkout of an attempt's baseline
//! manifest (head, uncommitted files, commits since the base) and records what it found.

use std::path::PathBuf;

use wg_exec::manifest::{RepoObservation, needs_commit_range, plan_capture};
use wg_git::{Git, GitError};
use wg_runtime::{App, index_written};
use wg_runtimespec::RequestContext;
use wg_runtimespec::effects::GIT_TIMEOUT_MS;
use wg_types::{Id, WgError};

use crate::spec::conclude::Captured;
use crate::spec::outcome::{capture_decision, read_entries, unprepared};

/// `capture_outcome`: records the outcome manifest, what the attempt modified, its change set and
/// the read-only violations. The attempt must have a prepared workspace.
pub fn capture_outcome(
    app: &mut App,
    context: &RequestContext,
    attempt_id: Id,
) -> Result<Captured, WgError> {
    let entries = app.runtime.store.read(|session| {
        let attempt = session.attempt_by_id(attempt_id)?.ok_or_else(|| {
            WgError::not_found(format!("attempt {attempt_id} has no prepared workspace"))
        })?;
        let baseline = match attempt.baseline_manifest_id {
            Some(id) => session.manifest_by_id(id)?,
            None => None,
        }
        .ok_or_else(|| unprepared(&attempt))?;
        let rows: Vec<_> = baseline.entries.iter().cloned().collect();
        read_entries(&rows)
    })?;
    // What git says about each checkout: its head, its uncommitted files, the commits since the base.
    let git_failure = |error: GitError| WgError::git(error.to_string());
    let observations = entries
        .iter()
        .map(|entry| {
            let git = Git {
                path: PathBuf::from(&entry.checkout_path),
                timeout_ms: GIT_TIMEOUT_MS,
            };
            let head = if git.is_repository().map_err(git_failure)? {
                git.head().map_err(git_failure)?
            } else {
                None
            };
            let changed_files = match &head {
                Some(_) => {
                    let status = git.status_porcelain().map_err(git_failure)?;
                    status
                        .staged
                        .into_iter()
                        .chain(status.unstaged)
                        .chain(status.untracked)
                        .collect()
                }
                None => Vec::new(),
            };
            let commits = match (&entry.sha, &head) {
                (Some(base), Some(tip)) if needs_commit_range(Some(base), Some(tip)) => {
                    git.rev_list_range(base, tip).map_err(git_failure)?
                }
                _ => Vec::new(),
            };
            Ok(RepoObservation {
                head,
                changed_files,
                commits,
            })
        })
        .collect::<Result<Vec<RepoObservation>, WgError>>()?;
    let commits: Vec<Vec<String>> = observations
        .iter()
        .map(|seen| seen.commits.clone())
        .collect();
    let outcome = app.runtime.decide_then(
        context,
        |session| {
            let attempt = session
                .attempt_by_id(attempt_id)?
                .ok_or_else(|| WgError::not_found(format!("attempt {attempt_id} not found")))?;
            let set = match session
                .change_sets_of_attempt(attempt_id)?
                .into_iter()
                .next()
            {
                Some(set) => {
                    let items = session.change_set_items(set.id)?;
                    Some((set, items))
                }
                None => None,
            };
            Ok((attempt, set))
        },
        |(attempt, set), stamp| {
            let plan = plan_capture(&entries, &observations, set.is_some());
            Ok(capture_decision(
                stamp,
                attempt,
                &plan,
                &commits,
                set.as_ref(),
            ))
        },
        index_written,
    )?;
    Ok(outcome.reply)
}
