//! Git fixture operations for scenario setup (CR-03).
//!
//! A scenario's `setup` steps name one of these operations: `init`, `commit`,
//! `write`, `branch`, `checkout`, `worktree`, `merge`, `delete`, `move`, `rebase`,
//! `cherry-pick` and `reset`. They run the same git
//! commands as `tests/conftest.py` (`GIT_ENV` and `GitRepo`): the author is
//! `Test Author`, the committer `Test Committer`, `core.autocrlf` is off, and
//! every commit or merge is dated `2026-01-01T00:MM:SS+00:00` from a tick that
//! counts the repository's commits, so one scenario gives the same hashes on
//! every run.

use crate::runner::{Outcome, run_process};
use std::path::{Path, PathBuf};
use std::process::Command;

const AUTHOR_NAME: &str = "Test Author";
const AUTHOR_EMAIL: &str = "author@example.com";
const COMMITTER_NAME: &str = "Test Committer";
const COMMITTER_EMAIL: &str = "committer@example.com";

/// A scenario repository and its commit tick. The tick lives in the caller so
/// that every setup step on the same repository continues the same count.
pub(crate) struct Repo<'a> {
    dir: PathBuf,
    tick: &'a mut u64,
}

impl<'a> Repo<'a> {
    pub fn new(dir: PathBuf, tick: &'a mut u64) -> Self {
        Self { dir, tick }
    }

    /// `git init -q -b main`, then the repository identity and `core.autocrlf=false`.
    pub fn init(&self) -> Result<Outcome, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("create {}: {e}", self.dir.display()))?;
        let init = self.git(&["init", "-q", "-b", "main"], None)?;
        if init.exit != 0 {
            return Ok(init);
        }
        let identity = [
            ("user.name", AUTHOR_NAME),
            ("user.email", AUTHOR_EMAIL),
            ("core.autocrlf", "false"),
        ];
        for (key, value) in identity {
            let set = self.git(&["config", key, value], None)?;
            if set.exit != 0 {
                return Ok(set);
            }
        }
        Ok(init)
    }

    /// Write `files` (creating parent directories), stage everything when
    /// `all_changes` is set, and commit with the next tick as its date.
    pub fn commit(
        &mut self,
        message: &str,
        files: &[(String, String)],
        all_changes: bool,
    ) -> Result<Outcome, String> {
        self.write_files(files)?;
        if all_changes {
            let added = self.git(&["add", "-A"], None)?;
            if added.exit != 0 {
                return Ok(added);
            }
        }
        let stamp = self.next_stamp();
        self.git(
            &["commit", "-q", "--allow-empty", "-m", message],
            Some(&stamp),
        )
    }

    /// Write `files` without touching git.
    pub fn write(&self, files: &[(String, String)]) -> Result<Outcome, String> {
        self.write_files(files)?;
        Ok(Outcome {
            exit: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    /// `git branch <name>` at the current HEAD.
    pub fn branch(&self, name: &str) -> Result<Outcome, String> {
        self.git(&["branch", name], None)
    }

    /// `git checkout -q <name>`, or `git checkout -q -b <name>` when `create` is set.
    pub fn checkout(&self, name: &str, create: bool) -> Result<Outcome, String> {
        if create {
            self.git(&["checkout", "-q", "-b", name], None)
        } else {
            self.git(&["checkout", "-q", name], None)
        }
    }

    /// `git worktree add -q <path> <branch>`; a relative `path` is taken from the repository.
    pub fn worktree(&self, path: &str, branch: &str) -> Result<Outcome, String> {
        self.git(&["worktree", "add", "-q", path, branch], None)
    }

    /// `git merge --no-edit -q <branch>`, dated with the next tick like a commit.
    pub fn merge(&mut self, branch: &str) -> Result<Outcome, String> {
        let stamp = self.next_stamp();
        self.git(&["merge", "--no-edit", "-q", branch], Some(&stamp))
    }

    /// `git rm -q -r -- <paths>`: remove paths from the work tree and the index. The removal
    /// is committed by a later `commit` step, which stages everything.
    pub fn delete(&self, paths: &[String]) -> Result<Outcome, String> {
        let mut args = vec!["rm", "-q", "-r", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.git(&args, None)
    }

    /// `git mv -- <from> <to>`. The destination's parent directories are created first, so a
    /// move into a new directory works the way the Python fixture's file moves do.
    pub fn move_path(&self, from: &str, to: &str) -> Result<Outcome, String> {
        let target = self.dir.join(to);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        self.git(&["mv", "--", from, to], None)
    }

    /// `git rebase -q <onto>`, dated with the next tick: the rewritten commits take that date.
    pub fn rebase(&mut self, onto: &str) -> Result<Outcome, String> {
        let stamp = self.next_stamp();
        self.git(&["rebase", "-q", onto], Some(&stamp))
    }

    /// `git cherry-pick <rev>`, dated with the next tick like a commit. (`cherry-pick` has no
    /// `-q`, unlike `merge` and `rebase`.)
    pub fn cherry_pick(&mut self, rev: &str) -> Result<Outcome, String> {
        let stamp = self.next_stamp();
        self.git(&["cherry-pick", rev], Some(&stamp))
    }

    /// `git reset -q [<mode>] <rev>`, where `mode` is `--soft`, `--mixed` or `--hard`.
    pub fn reset(&self, mode: Option<&str>, rev: &str) -> Result<Outcome, String> {
        let mut args = vec!["reset", "-q"];
        args.extend(mode);
        args.push(rev);
        self.git(&args, None)
    }

    fn write_files(&self, files: &[(String, String)]) -> Result<(), String> {
        for (name, content) in files {
            let path = self.dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create {}: {e}", parent.display()))?;
            }
            std::fs::write(&path, content).map_err(|e| format!("write {}: {e}", path.display()))?;
        }
        Ok(())
    }

    /// Advance the tick and format it as `tests/conftest.py` does.
    fn next_stamp(&mut self) -> String {
        *self.tick += 1;
        let t = *self.tick;
        format!("2026-01-01T00:{:02}:{:02}+00:00", t / 60, t % 60)
    }

    /// Run `git <args>` in the repository. `date` sets both commit dates.
    fn git(&self, args: &[&str], date: Option<&str>) -> Result<Outcome, String> {
        let mut cmd = Command::new("git");
        cmd.args(["-c", "core.autocrlf=false", "-c", "init.defaultBranch=main"])
            .args(args)
            .current_dir(&self.dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", global_config_for(&self.dir))
            .env("GIT_AUTHOR_NAME", AUTHOR_NAME)
            .env("GIT_AUTHOR_EMAIL", AUTHOR_EMAIL)
            .env("GIT_COMMITTER_NAME", COMMITTER_NAME)
            .env("GIT_COMMITTER_EMAIL", COMMITTER_EMAIL);
        if let Some(date) = date {
            cmd.env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date);
        }
        run_process(&mut cmd, None)
    }
}

/// `tests/conftest.py` points `GIT_CONFIG_GLOBAL` at a file beside the repository
/// so the user's own git configuration never reaches a fixture.
fn global_config_for(dir: &Path) -> PathBuf {
    dir.parent().unwrap_or(dir).join(".gitconfig-test")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!("wg-gitfix-{name}-{}-{nanos}", std::process::id()))
    }

    fn head_of(dir: &Path) -> String {
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default()
    }

    #[test]
    fn two_fresh_repositories_get_the_same_commit_hashes() {
        let mut heads = Vec::new();
        for run in ["a", "b"] {
            let dir = scratch(run);
            let mut tick = 0;
            let mut repo = Repo::new(dir.clone(), &mut tick);
            assert_eq!(repo.init().map(|o| o.exit), Ok(0));
            let files = vec![("README.md".to_owned(), "# fixture\n".to_owned())];
            assert_eq!(repo.commit("initial", &files, true).map(|o| o.exit), Ok(0));
            heads.push(head_of(&dir));
            let _ = std::fs::remove_dir_all(&dir);
        }
        assert!(!heads[0].is_empty());
        assert_eq!(heads[0], heads[1]);
    }

    #[test]
    fn tick_formats_like_the_python_fixture() {
        let mut tick = 61;
        let mut repo = Repo::new(PathBuf::from("unused"), &mut tick);
        assert_eq!(repo.next_stamp(), "2026-01-01T00:01:02+00:00");
        assert_eq!(tick, 62);
    }

    /// Run a git command in `dir` and return its trimmed stdout (panics on failure).
    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", global_config_for(dir))
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A fresh repository with a `base` commit holding `a.txt` and `b.txt`.
    fn fixture(name: &str) -> (PathBuf, u64) {
        let dir = scratch(name);
        let mut tick = 0;
        let mut repo = Repo::new(dir.clone(), &mut tick);
        assert_eq!(repo.init().map(|o| o.exit), Ok(0));
        let files = vec![
            (
                "a.txt".to_owned(),
                "a
"
                .to_owned(),
            ),
            (
                "b.txt".to_owned(),
                "b
"
                .to_owned(),
            ),
        ];
        assert_eq!(repo.commit("base", &files, true).map(|o| o.exit), Ok(0));
        (dir, tick)
    }

    fn exit_of(r: Result<Outcome, String>) -> i64 {
        r.expect("git step ran").exit
    }

    #[test]
    fn delete_removes_paths_once_committed() {
        let (dir, mut tick) = fixture("delete");
        let mut repo = Repo::new(dir.clone(), &mut tick);
        assert_eq!(exit_of(repo.delete(&["a.txt".to_owned()])), 0);
        assert_eq!(exit_of(repo.commit("drop a", &[], true)), 0);
        assert_eq!(git_out(&dir, &["ls-files"]), "b.txt");
        assert_eq!(exit_of(repo.delete(&["nope.txt".to_owned()])), 128);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn move_creates_destination_directories() {
        let (dir, mut tick) = fixture("move");
        let mut repo = Repo::new(dir.clone(), &mut tick);
        assert_eq!(exit_of(repo.move_path("a.txt", "sub/deep/c.txt")), 0);
        assert_eq!(exit_of(repo.commit("move a", &[], true)), 0);
        assert_eq!(
            git_out(&dir, &["ls-files"]),
            "b.txt
sub/deep/c.txt"
        );
        assert!(!dir.join("a.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reset_moves_the_branch_back_with_and_without_a_mode() {
        let (dir, mut tick) = fixture("reset");
        let mut repo = Repo::new(dir.clone(), &mut tick);
        let base = git_out(&dir, &["rev-parse", "HEAD"]);
        let files = vec![(
            "c.txt".to_owned(),
            "c
"
            .to_owned(),
        )];
        assert_eq!(exit_of(repo.commit("second", &files, true)), 0);
        assert_eq!(exit_of(repo.reset(Some("--hard"), "HEAD~1")), 0);
        assert_eq!(git_out(&dir, &["rev-parse", "HEAD"]), base);
        assert!(!dir.join("c.txt").exists());
        assert_eq!(exit_of(repo.commit("again", &files, true)), 0);
        assert_eq!(exit_of(repo.reset(None, &base)), 0);
        assert_eq!(git_out(&dir, &["rev-parse", "HEAD"]), base);
        // A mixed reset keeps the work tree, so c.txt is still on disk.
        assert!(dir.join("c.txt").exists());
        assert_eq!(exit_of(repo.reset(Some("--bogus"), "HEAD")), 129);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cherry_pick_applies_a_commit_with_a_fixed_date() {
        let (dir, mut tick) = fixture("cherry");
        let mut repo = Repo::new(dir.clone(), &mut tick);
        assert_eq!(exit_of(repo.checkout("feat", true)), 0);
        let files = vec![(
            "feat.txt".to_owned(),
            "f
"
            .to_owned(),
        )];
        assert_eq!(exit_of(repo.commit("feature", &files, true)), 0);
        let feature = git_out(&dir, &["rev-parse", "HEAD"]);
        assert_eq!(exit_of(repo.checkout("main", false)), 0);
        assert_eq!(exit_of(repo.cherry_pick(&feature)), 0);
        assert_eq!(
            git_out(&dir, &["ls-files"]),
            "a.txt
b.txt
feat.txt"
        );
        assert_eq!(
            git_out(&dir, &["log", "-1", "--format=%cI", "HEAD"]),
            "2026-01-01T00:00:03Z"
        );
        assert_eq!(exit_of(repo.cherry_pick("no-such-rev")), 128);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rebase_replays_a_branch_onto_the_new_tip() {
        let (dir, mut tick) = fixture("rebase");
        let mut repo = Repo::new(dir.clone(), &mut tick);
        assert_eq!(exit_of(repo.checkout("feat", true)), 0);
        let files = vec![(
            "feat.txt".to_owned(),
            "f
"
            .to_owned(),
        )];
        assert_eq!(exit_of(repo.commit("feature", &files, true)), 0);
        assert_eq!(exit_of(repo.checkout("main", false)), 0);
        let main_files = vec![(
            "main.txt".to_owned(),
            "m
"
            .to_owned(),
        )];
        assert_eq!(exit_of(repo.commit("main moves", &main_files, true)), 0);
        assert_eq!(exit_of(repo.checkout("feat", false)), 0);
        assert_eq!(exit_of(repo.rebase("main")), 0);
        assert_eq!(
            git_out(&dir, &["log", "--format=%s", "-3"]),
            "feature
main moves
base"
        );
        assert_eq!(
            git_out(&dir, &["ls-files"]),
            "a.txt
b.txt
feat.txt
main.txt"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
