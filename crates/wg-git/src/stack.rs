//! The git operations of the stack and worktree modules (`stack_integration.py`,
//! `stack_landing.py`, `stack_verification.py`, `worktrees.py`). Each names one git command with
//! the argument vector the Python code passes; where Python varies `check`, the caller passes the
//! [`RunOptions`] it used. Decisions on the output stay with the callers, parsing with `wg-gitcore`.

use wg_gitcore::{GitError, RunOptions, parse_path_lines};

use crate::runner::{Git, GitOutput};

/// The environment of stack merges (`INTEGRATION_IDENTITY`): the integration author and
/// committer, set on top of the E24 environment.
pub const INTEGRATION_IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Workgraph Stack Integration"),
    ("GIT_AUTHOR_EMAIL", "integration@workgraph.local"),
    ("GIT_COMMITTER_NAME", "Workgraph Stack Integration"),
    ("GIT_COMMITTER_EMAIL", "integration@workgraph.local"),
];

impl Git {
    /// `merge --no-ff --no-edit -m <message> <head>` as the integration identity, unchecked: the
    /// caller reads the return code and the conflicts.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn merge_no_ff(&self, message: &str, head: &str) -> Result<GitOutput, GitError> {
        self.run_with(
            &["merge", "--no-ff", "--no-edit", "-m", message, head],
            RunOptions {
                env: INTEGRATION_IDENTITY,
                ..RunOptions::UNCHECKED
            },
        )
    }

    /// `merge --ff-only <tip>`.
    ///
    /// # Errors
    /// [`GitError`] as `options` says.
    pub fn merge_ff_only(&self, tip: &str, options: RunOptions<'_>) -> Result<GitOutput, GitError> {
        self.run_with(&["merge", "--ff-only", tip], options)
    }

    /// `merge --abort`.
    ///
    /// # Errors
    /// [`GitError`] as `options` says.
    pub fn merge_abort(&self, options: RunOptions<'_>) -> Result<GitOutput, GitError> {
        self.run_with(&["merge", "--abort"], options)
    }

    /// `diff --name-only --diff-filter=U` unchecked: the paths with unresolved conflicts.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn unmerged_paths(&self) -> Result<Vec<String>, GitError> {
        let output = self.run_with(
            &["diff", "--name-only", "--diff-filter=U"],
            RunOptions::UNCHECKED,
        )?;
        Ok(parse_path_lines(&output.stdout))
    }

    /// `update-ref <reference> <new> <old>`. With `old` set, git moves the ref only when it still
    /// points at `old` (compare-and-swap); otherwise it fails.
    ///
    /// # Errors
    /// [`GitError`] as `options` says.
    pub fn update_ref(
        &self,
        reference: &str,
        new: &str,
        old: &str,
        options: RunOptions<'_>,
    ) -> Result<GitOutput, GitError> {
        self.run_with(&["update-ref", reference, new, old], options)
    }

    /// `reset --keep <target>`.
    ///
    /// # Errors
    /// [`GitError`] as `options` says.
    pub fn reset_keep(&self, target: &str, options: RunOptions<'_>) -> Result<GitOutput, GitError> {
        self.run_with(&["reset", "--keep", target], options)
    }

    /// `reset --hard <target>`.
    ///
    /// # Errors
    /// [`GitError`] as `options` says.
    pub fn reset_hard(&self, target: &str, options: RunOptions<'_>) -> Result<GitOutput, GitError> {
        self.run_with(&["reset", "--hard", target], options)
    }

    /// `clean -ffdx`: removes untracked files, directories and ignored files, including nested
    /// repositories.
    ///
    /// # Errors
    /// [`GitError`] as `options` says.
    pub fn clean_force(&self, options: RunOptions<'_>) -> Result<GitOutput, GitError> {
        self.run_with(&["clean", "-ffdx"], options)
    }

    /// `checkout --force --detach <sha>`.
    ///
    /// # Errors
    /// [`GitError`] as `options` says.
    pub fn checkout_detach_force(
        &self,
        sha: &str,
        options: RunOptions<'_>,
    ) -> Result<GitOutput, GitError> {
        self.run_with(&["checkout", "--force", "--detach", sha], options)
    }

    /// `branch -D <branch>`.
    ///
    /// # Errors
    /// [`GitError`] as `options` says.
    pub fn branch_delete(
        &self,
        branch: &str,
        options: RunOptions<'_>,
    ) -> Result<GitOutput, GitError> {
        self.run_with(&["branch", "-D", branch], options)
    }

    /// `worktree add -b <branch> <path> <base>`.
    ///
    /// # Errors
    /// [`GitError::Failed`] when git refuses.
    pub fn worktree_add_branch(
        &self,
        branch: &str,
        path: &str,
        base: &str,
    ) -> Result<GitOutput, GitError> {
        self.run_with(
            &["worktree", "add", "-b", branch, path, base],
            RunOptions::CHECKED,
        )
    }

    /// `worktree add --detach <path> <base>`.
    ///
    /// # Errors
    /// [`GitError::Failed`] when git refuses.
    pub fn worktree_add_detached(&self, path: &str, base: &str) -> Result<GitOutput, GitError> {
        self.run_with(
            &["worktree", "add", "--detach", path, base],
            RunOptions::CHECKED,
        )
    }

    /// `worktree add <path> <branch>`: checks out an existing branch in a new worktree.
    ///
    /// # Errors
    /// [`GitError::Failed`] when git refuses.
    pub fn worktree_add_existing(&self, path: &str, branch: &str) -> Result<GitOutput, GitError> {
        self.run_with(&["worktree", "add", path, branch], RunOptions::CHECKED)
    }

    /// `worktree remove [--force] <path>`.
    ///
    /// # Errors
    /// [`GitError`] as `options` says.
    pub fn worktree_remove(
        &self,
        path: &str,
        force: bool,
        options: RunOptions<'_>,
    ) -> Result<GitOutput, GitError> {
        if force {
            self.run_with(&["worktree", "remove", "--force", path], options)
        } else {
            self.run_with(&["worktree", "remove", path], options)
        }
    }

    /// `worktree prune`.
    ///
    /// # Errors
    /// [`GitError`] as `options` says.
    pub fn worktree_prune(&self, options: RunOptions<'_>) -> Result<GitOutput, GitError> {
        self.run_with(&["worktree", "prune"], options)
    }
}
