//! The managed worktrees of a stack: the integration worktree of each repository, the detached
//! worktrees that probe prefixes, and their removal (`ensure_worktree`, `_bisect_worktree`,
//! `WorktreeService.remove`).

use std::path::PathBuf;

use crate::spec::exec::worktree::{
    IntegrationTarget, Purpose, bisect_path, integration_path, worktree_active,
};
use wg_git::RunOptions;
use wg_runtimespec::RequestContext;
use wg_types::{Id, WgError, WorktreeStatus};

use crate::spec::{git_failure, io_error, native_root};

use super::Exec;

impl Exec<'_> {
    /// `StackIntegrator.ensure_worktree`: the integration worktree of a stack repository, created
    /// on first use.
    pub(crate) fn ensure_worktree(
        &mut self,
        context: &RequestContext,
        target: &IntegrationTarget,
    ) -> Result<PathBuf, WgError> {
        let path = integration_path(
            &native_root(self.0.paths.worktrees_dir().as_str()),
            target.stack_id,
            &target.repository_name,
        );
        let text = path.to_string_lossy().into_owned();
        let existing = self
            .0
            .runtime
            .store
            .read(|session| session.worktree_by_path(&text))?;
        let usable = existing
            .as_ref()
            .is_some_and(|row| row.status == WorktreeStatus::Active)
            && path.exists()
            && git_at!(&path).is_repository().unwrap_or(false);
        if usable {
            return Ok(path);
        }
        let source = git_at!(std::path::Path::new(&target.source_path));
        source
            .worktree_prune(RunOptions::UNCHECKED)
            .map_err(|err| git_failure(&err))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| io_error(&err))?;
        }
        let branch = target.integration_branch.as_str();
        let reference = format!("refs/heads/{branch}");
        let found = source
            .run_with(
                &["rev-parse", "--verify", "--quiet", &reference],
                RunOptions::UNCHECKED,
            )
            .map_err(|err| git_failure(&err))?;
        let added = if found.returncode == 0 {
            source.worktree_add_existing(&text, branch)
        } else {
            source.worktree_add_branch(branch, &text, &target.base_sha)
        };
        added.map_err(|err| git_failure(&err))?;
        self.0.runtime.decide(
            context,
            |session| session.worktree_by_path(&text),
            |row, stamp| {
                Ok(worktree_active(
                    row.as_ref(),
                    target,
                    &text,
                    Purpose::Integration,
                    stamp,
                ))
            },
        )?;
        Ok(path)
    }

    /// `StackVerifier._bisect_worktree`: a detached worktree for probing prefixes of one
    /// repository; returns its row id and path.
    pub(crate) fn bisect_worktree(
        &mut self,
        context: &RequestContext,
        target: &IntegrationTarget,
    ) -> Result<(Id, PathBuf), WgError> {
        let path = bisect_path(
            &native_root(self.0.paths.worktrees_dir().as_str()),
            target.stack_id,
            &target.repository_name,
        );
        let text = path.to_string_lossy().into_owned();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| io_error(&err))?;
        }
        let source = git_at!(std::path::Path::new(&target.source_path));
        source
            .worktree_prune(RunOptions::UNCHECKED)
            .map_err(|err| git_failure(&err))?;
        if path.exists() {
            source
                .worktree_remove(&text, true, RunOptions::UNCHECKED)
                .map_err(|err| git_failure(&err))?;
            // The directory may remain when git refused; clear it either way.
            let _ = std::fs::remove_dir_all(&path);
        }
        source
            .worktree_add_detached(&text, &target.base_sha)
            .map_err(|err| git_failure(&err))?;
        let row_id = self
            .0
            .runtime
            .decide(
                context,
                |session| session.worktree_by_path(&text),
                |row, stamp| {
                    Ok(worktree_active(
                        row.as_ref(),
                        target,
                        &text,
                        Purpose::Bisect,
                        stamp,
                    ))
                },
            )?
            .reply;
        Ok((row_id, path))
    }

    /// `WorktreeService.remove(force=True)`: the worktree is removed from disk and its row marked,
    /// by the git feature's worktree service (the one implementation of the removal).
    pub(crate) fn remove_worktree(
        &mut self,
        context: &RequestContext,
        row_id: Id,
    ) -> Result<(), WgError> {
        wg_git_feature::service::worktree_remove::worktree_remove(self.0, context, row_id, true)
            .map(|_| ())
    }
}
