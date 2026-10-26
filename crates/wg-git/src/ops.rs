//! The git operations of `integrations/git/service.py`: each one runs one git command and returns
//! its text (or the ids in it) for `wg-gitcore` to parse. The argument shapes are the Python ones.

use wg_gitcore::{GitError, RunOptions, non_empty_trimmed, split_words, stdin_lines};

use crate::runner::{Git, GitOutput};

impl Git {
    /// `rev-list --all`: every commit reachable from any ref.
    ///
    /// # Errors
    /// [`GitError`] when the call fails.
    pub fn rev_list_all(&self) -> Result<Vec<String>, GitError> {
        let output = self.run_with(&["rev-list", "--all"], RunOptions::CHECKED)?;
        Ok(split_words(&output.stdout))
    }

    /// `rev-list --all --stdin` fed `^<tip>` lines: the commits reachable from a ref but not from
    /// any of `excluded`.
    ///
    /// # Errors
    /// [`GitError`] when the call fails.
    pub fn rev_list_all_excluding(&self, excluded: &[String]) -> Result<Vec<String>, GitError> {
        let negated: Vec<String> = excluded.iter().map(|sha| format!("^{sha}")).collect();
        let input = stdin_lines(&negated);
        let output = self.run_with(
            &["rev-list", "--all", "--stdin"],
            RunOptions {
                stdin: Some(&input),
                ..RunOptions::CHECKED
            },
        )?;
        Ok(split_words(&output.stdout))
    }

    /// `rev-list --reverse <base>..<head> --`: the commits of a range, oldest first.
    ///
    /// # Errors
    /// [`GitError`] when the call fails.
    pub fn rev_list_range(&self, base: &str, head: &str) -> Result<Vec<String>, GitError> {
        let range = format!("{base}..{head}");
        let output = self.run_with(
            &["rev-list", "--reverse", range.as_str(), "--"],
            RunOptions::CHECKED,
        )?;
        Ok(split_words(&output.stdout))
    }

    /// `rev-list <args>` unchecked: the caller reads the return code and the ids.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn rev_list(&self, args: &[&str]) -> Result<GitOutput, GitError> {
        let mut full = vec!["rev-list"];
        full.extend_from_slice(args);
        self.run_with(&full, RunOptions::UNCHECKED)
    }

    /// `log --no-walk=unsorted --stdin --format=META_FORMAT` fed the commit ids: the metadata
    /// records `wg_gitcore::parse_meta` reads.
    ///
    /// # Errors
    /// [`GitError`] when the call fails.
    pub fn log_meta(&self, shas: &[String]) -> Result<String, GitError> {
        let format = format!("--format={}", wg_gitcore::META_FORMAT);
        let input = stdin_lines(shas);
        let output = self.run_with(
            &["log", "--no-walk=unsorted", "--stdin", format.as_str()],
            RunOptions {
                stdin: Some(&input),
                ..RunOptions::CHECKED
            },
        )?;
        Ok(output.stdout)
    }

    /// `diff-tree --stdin -z -r -M -C --root --raw --numstat` fed `<sha> <parent>` lines: the raw
    /// and numstat records `wg_gitcore::parse_diff_tree` reads.
    ///
    /// # Errors
    /// [`GitError`] when the call fails.
    pub fn diff_tree_raw_numstat(&self, lines: &[String]) -> Result<String, GitError> {
        let input = stdin_lines(lines);
        let output = self.run_with(
            &[
                "diff-tree",
                "--stdin",
                "-z",
                "-r",
                "-M",
                "-C",
                "--root",
                "--raw",
                "--numstat",
            ],
            RunOptions {
                stdin: Some(&input),
                ..RunOptions::CHECKED
            },
        )?;
        Ok(output.stdout)
    }

    /// `diff-tree --stdin -p -r --root` fed commit lines: the patches the patch id is computed on.
    ///
    /// # Errors
    /// [`GitError`] when the call fails.
    pub fn diff_tree_patches(&self, lines: &[String]) -> Result<String, GitError> {
        let input = stdin_lines(lines);
        let output = self.run_with(
            &["diff-tree", "--stdin", "-p", "-r", "--root"],
            RunOptions {
                stdin: Some(&input),
                ..RunOptions::CHECKED
            },
        )?;
        Ok(output.stdout)
    }

    /// `patch-id --stable` fed patches: the output `wg_gitcore::parse_patch_ids` reads.
    ///
    /// # Errors
    /// [`GitError`] when the call fails.
    pub fn patch_ids(&self, patches: &str) -> Result<String, GitError> {
        let output = self.run_with(
            &["patch-id", "--stable"],
            RunOptions {
                stdin: Some(patches),
                ..RunOptions::CHECKED
            },
        )?;
        Ok(output.stdout)
    }

    /// `diff -U0 --no-color -M <base> <head> -- <paths>` unchecked: the hunks of one file, or of
    /// every file when `paths` is empty.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn diff_unified(
        &self,
        base: &str,
        head: &str,
        paths: &[&str],
    ) -> Result<GitOutput, GitError> {
        let mut args = vec!["diff", "-U0", "--no-color", "-M", base, head, "--"];
        args.extend_from_slice(paths);
        self.run_with(&args, RunOptions::UNCHECKED)
    }

    /// `diff --numstat -z -M <base> <head>`: the numstat records `wg_gitcore::parse_numstat_z`
    /// reads.
    ///
    /// # Errors
    /// [`GitError`] when the call fails.
    pub fn diff_numstat_z(&self, base: &str, head: &str) -> Result<String, GitError> {
        let output = self.run_with(
            &["diff", "--numstat", "-z", "-M", base, head],
            RunOptions::CHECKED,
        )?;
        Ok(output.stdout)
    }

    /// `diff -M <base> <head>`: the full patch between two trees or commits.
    ///
    /// # Errors
    /// [`GitError`] when the call fails.
    pub fn diff_patch(&self, base: &str, head: &str) -> Result<String, GitError> {
        let output = self.run_with(&["diff", "-M", base, head], RunOptions::CHECKED)?;
        Ok(output.stdout)
    }

    /// `show --format=commit %H%n%s%n -M --first-parent <sha>`: the header and patch of a commit.
    ///
    /// # Errors
    /// [`GitError`] when the call fails.
    pub fn show_commit(&self, sha: &str) -> Result<String, GitError> {
        let output = self.run_with(
            &[
                "show",
                "--format=commit %H%n%s%n",
                "-M",
                "--first-parent",
                sha,
            ],
            RunOptions::CHECKED,
        )?;
        Ok(output.stdout)
    }

    /// `rev-parse --verify --quiet <rev>`: the commit id, `None` when `rev` does not resolve.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run.
    pub fn rev_parse_verify_quiet(&self, rev: &str) -> Result<Option<String>, GitError> {
        let output = self.run_with(
            &["rev-parse", "--verify", "--quiet", rev],
            RunOptions::UNCHECKED,
        )?;
        Ok(non_empty_trimmed(&output.stdout))
    }
}
