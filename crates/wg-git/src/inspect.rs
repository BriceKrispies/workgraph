//! The inspection half of `integrations/git/cli.py`'s `Git` class: repository location, refs,
//! branches, object existence and the working tree status. Each method runs git and hands the
//! text to `wg-gitcore` for parsing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wg_gitcore::{
    GitError, RunOptions, WorkingTreeStatus, WorktreeEntry, choose_default_branch, is_sha,
    is_valid_ref, non_empty_trimmed, parse_refs, parse_remotes, parse_sha_lines,
    parse_status_porcelain_v2, parse_worktree_porcelain, strip_verbatim_prefix,
};

use crate::runner::{Git, git_version};

/// Resolves a path the way Python's `Path.resolve()` does: absolute, symlinks followed, and the
/// Windows verbatim prefix left out. A path that cannot be resolved is returned unchanged.
pub(crate) fn resolve(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(resolved) => PathBuf::from(strip_verbatim_prefix(&resolved.display().to_string())),
        Err(_) => path.to_path_buf(),
    }
}

impl Git {
    /// Whether the path is a repository or a worktree of one (bare repositories included).
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn is_repository(&self) -> Result<bool, GitError> {
        if !self.path.exists() {
            return Ok(false);
        }
        let inside = self.run_with(
            &["rev-parse", "--is-inside-work-tree"],
            RunOptions::UNCHECKED,
        )?;
        if inside.returncode == 0 {
            return Ok(true);
        }
        let bare = self.run_with(
            &["rev-parse", "--is-bare-repository"],
            RunOptions::UNCHECKED,
        )?;
        Ok(bare.returncode == 0 && bare.stdout.trim() == "true")
    }

    /// The top of the working tree, resolved.
    ///
    /// # Errors
    /// [`GitError`] when the rev-parse call fails.
    pub fn toplevel(&self) -> Result<PathBuf, GitError> {
        let value = self.run_with(&["rev-parse", "--show-toplevel"], RunOptions::CHECKED)?;
        Ok(resolve(Path::new(value.stdout.trim())))
    }

    /// The common git directory: a relative answer is taken from the runner's path.
    ///
    /// # Errors
    /// [`GitError`] when the rev-parse call fails.
    pub fn git_common_dir(&self) -> Result<PathBuf, GitError> {
        let value = self.run_with(&["rev-parse", "--git-common-dir"], RunOptions::CHECKED)?;
        let candidate = PathBuf::from(value.stdout.trim());
        if candidate.is_absolute() {
            Ok(candidate)
        } else {
            Ok(resolve(&self.path.join(candidate)))
        }
    }

    /// Whether this path is a linked worktree rather than the main working tree.
    ///
    /// # Errors
    /// [`GitError`] when a rev-parse call fails.
    pub fn is_worktree(&self) -> Result<bool, GitError> {
        let git_dir = self.run_with(&["rev-parse", "--absolute-git-dir"], RunOptions::CHECKED)?;
        let common = self.run_with(&["rev-parse", "--git-common-dir"], RunOptions::CHECKED)?;
        let common = PathBuf::from(common.stdout.trim());
        let common = if common.is_absolute() {
            common
        } else {
            self.path.join(common)
        };
        Ok(resolve(Path::new(git_dir.stdout.trim())) != resolve(&common))
    }

    /// Every worktree of the repository, the main working tree first.
    ///
    /// # Errors
    /// [`GitError`] when the worktree list cannot be read.
    pub fn worktrees(&self) -> Result<Vec<WorktreeEntry>, GitError> {
        if git_version() >= (2, 36) {
            let output = self.run_with(
                &["worktree", "list", "--porcelain", "-z"],
                RunOptions::CHECKED,
            )?;
            Ok(parse_worktree_porcelain(&output.stdout, true))
        } else {
            let output =
                self.run_with(&["worktree", "list", "--porcelain"], RunOptions::CHECKED)?;
            Ok(parse_worktree_porcelain(&output.stdout, false))
        }
    }

    /// The commit at HEAD, `None` before the first commit.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn head(&self) -> Result<Option<String>, GitError> {
        let output = self.run_with(
            &["rev-parse", "--verify", "--quiet", "HEAD"],
            RunOptions::UNCHECKED,
        )?;
        Ok(non_empty_trimmed(&output.stdout))
    }

    /// The short name of the branch HEAD points at, `None` when detached.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn current_branch(&self) -> Result<Option<String>, GitError> {
        let output = self.run_with(
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
            RunOptions::UNCHECKED,
        )?;
        Ok(non_empty_trimmed(&output.stdout))
    }

    /// Commits with no parents across all refs, sorted and unique. Empty when git fails.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn root_commits(&self) -> Result<Vec<String>, GitError> {
        let output = self.run_with(
            &["rev-list", "--max-parents=0", "--all"],
            RunOptions::UNCHECKED,
        )?;
        if output.returncode != 0 {
            return Ok(Vec::new());
        }
        Ok(parse_sha_lines(&output.stdout))
    }

    /// The fetch URL of each remote, in order.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn remotes(&self) -> Result<Vec<(String, String)>, GitError> {
        let output = self.run_with(&["remote", "-v"], RunOptions::UNCHECKED)?;
        Ok(parse_remotes(&output.stdout))
    }

    /// The default branch, as `WorkingTreeGit.default_branch` picks it (see
    /// `wg_gitcore::choose_default_branch` for the order).
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn default_branch(&self) -> Result<Option<String>, GitError> {
        let mut remote_heads = Vec::new();
        for (remote, _) in self.remotes()? {
            let reference = format!("refs/remotes/{remote}/HEAD");
            let output = self.run_with(
                &["symbolic-ref", "--quiet", reference.as_str()],
                RunOptions::UNCHECKED,
            )?;
            remote_heads.push(if output.returncode == 0 {
                output.stdout.trim().to_owned()
            } else {
                String::new()
            });
        }
        let configured = self.run_with(
            &["config", "--get", "init.defaultBranch"],
            RunOptions::UNCHECKED,
        )?;
        let branches = self.local_branches()?;
        let current = self.current_branch()?;
        Ok(choose_default_branch(
            &remote_heads,
            configured.stdout.trim(),
            &branches,
            current.as_deref(),
        ))
    }

    /// The short names of the local branches.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn local_branches(&self) -> Result<Vec<String>, GitError> {
        let output = self.run_with(
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
            RunOptions::UNCHECKED,
        )?;
        Ok(output
            .stdout
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Every branch, tag and remote-tracking ref to its commit (annotated tags peeled).
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn refs(&self) -> Result<BTreeMap<String, String>, GitError> {
        let output = self.run_with(
            &[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)%00%(*objectname)",
                "refs/heads",
                "refs/tags",
                "refs/remotes",
            ],
            RunOptions::UNCHECKED,
        )?;
        Ok(parse_refs(&output.stdout))
    }

    /// Whether the repository is shallow.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn is_shallow(&self) -> Result<bool, GitError> {
        let output = self.run_with(
            &["rev-parse", "--is-shallow-repository"],
            RunOptions::UNCHECKED,
        )?;
        Ok(output.stdout.trim() == "true")
    }

    /// Whether `sha` names a commit this repository has. Anything that is not a commit id is
    /// `false` without asking git.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn object_exists(&self, sha: &str) -> Result<bool, GitError> {
        if !is_sha(sha) {
            return Ok(false);
        }
        let spec = format!("{sha}^{{commit}}");
        let output = self.run_with(&["cat-file", "-e", spec.as_str()], RunOptions::UNCHECKED)?;
        Ok(output.returncode == 0)
    }

    /// The commit id of `reference`.
    ///
    /// # Errors
    /// [`GitError::InvalidRef`] for a name git must not see, [`GitError`] when it does not resolve.
    pub fn resolve_commit(&self, reference: &str) -> Result<String, GitError> {
        if !is_valid_ref(reference) {
            return Err(GitError::InvalidRef(reference.to_owned()));
        }
        let spec = format!("{reference}^{{commit}}");
        let output = self.run_with(
            &["rev-parse", "--verify", spec.as_str()],
            RunOptions::CHECKED,
        )?;
        Ok(output.stdout.trim().to_owned())
    }

    /// Whether `ancestor` is an ancestor of `descendant` (or the same commit).
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, GitError> {
        let output = self.run_with(
            &["merge-base", "--is-ancestor", ancestor, descendant],
            RunOptions::UNCHECKED,
        )?;
        Ok(output.returncode == 0)
    }

    /// The content of `path` at commit `sha`, `None` when it is absent or git fails.
    pub fn show_file(&self, sha: &str, path: &str) -> Option<Vec<u8>> {
        let spec = format!("{sha}:{path}");
        self.raw_output(&["show", spec.as_str()])
    }

    /// The working tree status (`status --porcelain=v2 --branch -z`).
    ///
    /// # Errors
    /// [`GitError`] when the status call fails.
    pub fn status_porcelain(&self) -> Result<WorkingTreeStatus, GitError> {
        let output = self.run_with(
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=normal",
            ],
            RunOptions::CHECKED,
        )?;
        Ok(parse_status_porcelain_v2(&output.stdout))
    }

    /// Whether the working tree has no staged, unstaged or untracked changes.
    ///
    /// # Errors
    /// [`GitError`] when the status call fails.
    pub fn is_clean(&self) -> Result<bool, GitError> {
        let output = self.run_with(
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=normal",
            ],
            RunOptions::CHECKED,
        )?;
        let status = parse_status_porcelain_v2(&output.stdout);
        Ok(status.staged.is_empty() && status.unstaged.is_empty() && status.untracked.is_empty())
    }
}
