//! Real-git tests for wg-git: every operation runs against repositories generated in a temporary
//! directory, as the Python tests use `tests/conftest.py`'s `GitRepo`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use wg_git::{Git, GitError, GitOutput, HookOutcome, RunOptions, install_post_commit_hook};
use wg_gitcore::{HOOK_MARKER, parse_meta};

/// The runner for a repository at `path`, with the default timeout.
fn repo(path: impl Into<PathBuf>) -> Git {
    Git {
        path: path.into(),
        timeout_ms: 120_000,
    }
}

/// A checked call with the default options, as the Python `Git.run` reads.
trait RunChecked {
    fn run(&self, args: &[&str]) -> Result<GitOutput, GitError>;
}

impl RunChecked for Git {
    fn run(&self, args: &[&str]) -> Result<GitOutput, GitError> {
        self.run_with(args, RunOptions::CHECKED)
    }
}

/// A fresh directory for `tag`, empty on return.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-git-sh15-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch directory");
    dir
}

/// Runs git directly, for fixtures only.
fn fixture(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00+00:00")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "fixture git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository on `main` with one commit writing `file.txt`. Returns its commit id.
fn repo_with_commit(dir: &Path) -> String {
    fixture(dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("file.txt"), "one\ntwo\n").expect("write file");
    fixture(dir, &["add", "file.txt"]);
    fixture(
        dir,
        &["commit", "-q", "-m", "first commit\n\nTask-ID: WG-1"],
    );
    fixture(dir, &["rev-parse", "HEAD"])
}

#[test]
fn runs_git_with_the_repository_as_working_directory() {
    let dir = scratch("run");
    let head = repo_with_commit(&dir);
    let git = Git {
        path: dir.clone(),
        timeout_ms: 120_000,
    };
    let output = git.run(&["rev-parse", "HEAD"]).unwrap();
    assert_eq!(output.returncode, 0);
    assert_eq!(output.stdout.trim(), head);
    assert_eq!(git.head().unwrap().as_deref(), Some(head.as_str()));
    assert_eq!(git.current_branch().unwrap().as_deref(), Some("main"));
}

#[test]
fn checked_failure_carries_the_python_message() {
    let dir = scratch("failure");
    repo_with_commit(&dir);
    let git = repo(&dir);
    let error = git
        .run(&["rev-parse", "--verify", "no-such-ref"])
        .unwrap_err();
    assert!(matches!(error, GitError::Failed { returncode, .. } if returncode != 0));
    assert!(
        error
            .to_string()
            .starts_with("git rev-parse --verify no-such-ref failed ("),
        "{error}"
    );
    let unchecked = git
        .run_with(
            &["rev-parse", "--verify", "no-such-ref"],
            RunOptions::UNCHECKED,
        )
        .unwrap();
    assert_ne!(unchecked.returncode, 0);
}

#[test]
fn repository_detection_and_location() {
    let dir = scratch("detect");
    repo_with_commit(&dir);
    let git = repo(&dir);
    assert!(git.is_repository().unwrap());
    assert!(!repo(dir.join("missing")).is_repository().unwrap());
    let plain = scratch("plain");
    assert!(!repo(&plain).is_repository().unwrap());
    assert!(!git.is_worktree().unwrap());
    // resolved, with the Windows verbatim prefix left out
    let top = git.toplevel().unwrap();
    assert!(!top.display().to_string().starts_with(r"\\?\"), "{top:?}");
    assert_eq!(
        fs::canonicalize(&top).unwrap(),
        fs::canonicalize(&dir).unwrap()
    );
    assert!(git.git_common_dir().unwrap().ends_with(".git"));
}

#[test]
fn refs_branches_and_commits() {
    let dir = scratch("refs");
    let first = repo_with_commit(&dir);
    fixture(&dir, &["tag", "-a", "v1", "-m", "release"]);
    fixture(&dir, &["branch", "feature"]);
    let git = repo(&dir);

    // ordered as git prints them (for-each-ref sorts by name), annotated tags peeled
    let refs = git.refs().unwrap();
    let names: Vec<&str> = refs.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        ["refs/heads/feature", "refs/heads/main", "refs/tags/v1"]
    );
    assert_eq!(refs[1], ("refs/heads/main".to_owned(), first.clone()));
    assert_eq!(
        refs[2],
        ("refs/tags/v1".to_owned(), first.clone()),
        "annotated tags are peeled"
    );

    let mut branches = git.local_branches().unwrap();
    branches.sort();
    assert_eq!(branches, ["feature", "main"]);
    assert_eq!(git.default_branch().unwrap().as_deref(), Some("main"));
    assert_eq!(git.root_commits().unwrap(), std::slice::from_ref(&first));
    assert!(git.object_exists(&first).unwrap());
    assert!(!git.object_exists("not-a-sha").unwrap());
    assert!(!git.is_shallow().unwrap());
    assert_eq!(git.resolve_commit("main").unwrap(), first);
    assert!(matches!(
        git.resolve_commit("--bad"),
        Err(GitError::InvalidRef(_))
    ));
    assert!(git.is_ancestor(&first, "main").unwrap());
    assert_eq!(git.rev_parse_verify_quiet("no-such").unwrap(), None);
    assert_eq!(
        git.rev_parse_verify_quiet("main").unwrap().as_deref(),
        Some(first.as_str())
    );
}

#[test]
fn working_tree_status_and_worktrees() {
    let dir = scratch("status");
    repo_with_commit(&dir);
    let git = repo(&dir);
    assert!(git.is_clean().unwrap());

    fs::write(dir.join("untracked.txt"), "x").unwrap();
    assert!(!git.is_clean().unwrap());
    assert_eq!(git.status_porcelain().unwrap().untracked, ["untracked.txt"]);
    fs::remove_file(dir.join("untracked.txt")).unwrap();

    let linked = scratch("status-linked").join("linked");
    fixture(
        &dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "linked-branch-2",
            linked.to_str().unwrap(),
        ],
    );
    let entries = git.worktrees().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].branch.as_deref(), Some("main"));
    assert!(repo(&linked).is_worktree().unwrap());
    assert!(!git.is_worktree().unwrap());
}

#[test]
fn remotes_and_shows_files() {
    let dir = scratch("remote");
    let first = repo_with_commit(&dir);
    fixture(
        &dir,
        &["remote", "add", "origin", "https://example.invalid/x.git"],
    );
    let git = repo(&dir);
    assert_eq!(
        git.remotes().unwrap(),
        [(
            "origin".to_owned(),
            "https://example.invalid/x.git".to_owned()
        )]
    );
    assert_eq!(
        git.show_file(&first, "file.txt"),
        Some(b"one\ntwo\n".to_vec())
    );
    assert_eq!(git.show_file(&first, "absent.txt"), None);
}

#[test]
fn commit_lists_logs_and_diffs() {
    let dir = scratch("ops");
    let first = repo_with_commit(&dir);
    fs::write(dir.join("file.txt"), "one\nthree\n").unwrap();
    fixture(&dir, &["commit", "-q", "-am", "second"]);
    let second = fixture(&dir, &["rev-parse", "HEAD"]);
    let git = repo(&dir);

    assert_eq!(git.rev_list_all().unwrap(), [second.clone(), first.clone()]);
    assert_eq!(
        git.rev_list_all_excluding(std::slice::from_ref(&first))
            .unwrap(),
        std::slice::from_ref(&second)
    );
    assert_eq!(
        git.rev_list_range(&first, &second).unwrap(),
        std::slice::from_ref(&second)
    );
    assert_eq!(
        git.rev_list(&["--count", "main"]).unwrap().stdout.trim(),
        "2"
    );

    let meta = parse_meta(&git.log_meta(std::slice::from_ref(&second)).unwrap()).unwrap();
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0].sha, second);
    assert_eq!(meta[0].parents, std::slice::from_ref(&first));

    let numstat = git.diff_numstat_z(&first, &second).unwrap();
    assert!(numstat.contains("file.txt"), "{numstat}");
    let hunks = git
        .diff_unified(&first, &second, &["file.txt"])
        .unwrap()
        .stdout;
    assert!(hunks.contains("+three"), "{hunks}");
    assert!(git.diff_patch(&first, &second).unwrap().contains("+three"));
    assert!(git.show_commit(&second).unwrap().contains("second"));
    let raw = git
        .diff_tree_raw_numstat(&[format!("{second} {first}")])
        .unwrap();
    assert!(raw.contains("file.txt"), "{raw}");
    assert!(
        git.diff_tree_patches(std::slice::from_ref(&second))
            .unwrap()
            .contains("+three")
    );
    assert!(git.patch_ids("").unwrap().is_empty());
}

#[test]
fn post_commit_hook_is_installed_and_never_overwrites_a_foreign_hook() {
    let dir = scratch("hook");
    let repo_id = "repo-hook-1";
    repo_with_commit(&dir);
    let git = repo(&dir);

    let outcome = install_post_commit_hook(&git, repo_id).unwrap();
    let HookOutcome::Installed(path) = outcome else {
        panic!("a fresh repository gets the hook");
    };
    let script = fs::read_to_string(&path).unwrap();
    assert!(script.contains(HOOK_MARKER));
    assert!(script.contains(&format!(r#""repository_id": "{repo_id}""#)));
    assert!(path.ends_with("post-commit"));

    // a Workgraph-managed hook is replaced
    assert!(matches!(
        install_post_commit_hook(&git, repo_id).unwrap(),
        HookOutcome::Installed(_)
    ));

    // a foreign hook is refused and left as it was
    fs::write(&path, "#!/bin/sh\necho mine\n").unwrap();
    let refused = install_post_commit_hook(&git, repo_id).unwrap();
    assert_eq!(refused, HookOutcome::Refused(path.clone()));
    assert_eq!(fs::read_to_string(&path).unwrap(), "#!/bin/sh\necho mine\n");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(&path, "").unwrap();
        install_post_commit_hook(&git, repo_id).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}

#[test]
fn git_version_is_read() {
    assert!(wg_git::git_version() >= (1, 0));
}

/// Two branches off `main` that both change `file.txt` (a conflict), and `other.txt` on `feature`
/// only. Leaves `main` checked out.
fn diverged_repo(dir: &Path) -> (String, String) {
    let first = repo_with_commit(dir);
    fixture(dir, &["checkout", "-q", "-b", "feature"]);
    fs::write(dir.join("file.txt"), "feature\n").unwrap();
    fs::write(dir.join("other.txt"), "other\n").unwrap();
    fixture(dir, &["add", "."]);
    fixture(dir, &["commit", "-q", "-m", "feature change"]);
    fixture(dir, &["checkout", "-q", "main"]);
    fs::write(dir.join("file.txt"), "main\n").unwrap();
    fixture(dir, &["commit", "-q", "-am", "main change"]);
    let main_head = fixture(dir, &["rev-parse", "HEAD"]);
    (first, main_head)
}

#[test]
fn merge_conflict_is_reported_and_aborted() {
    let dir = scratch("merge-conflict");
    let (_, main_head) = diverged_repo(&dir);
    let git = repo(&dir);

    let merged = git.merge_no_ff("Merge feature", "feature").unwrap();
    assert_ne!(merged.returncode, 0);
    assert_eq!(git.unmerged_paths().unwrap(), ["file.txt"]);
    git.merge_abort(RunOptions::CHECKED).unwrap();
    assert!(git.is_clean().unwrap());
    assert_eq!(git.head().unwrap(), Some(main_head));
    assert!(git.unmerged_paths().unwrap().is_empty());
}

#[test]
fn merge_no_ff_uses_the_integration_identity_and_message() {
    let dir = scratch("merge-ok");
    let (first, _) = diverged_repo(&dir);
    // a non-conflicting feature: only other.txt, so the merge is clean
    fixture(&dir, &["checkout", "-q", "feature"]);
    fixture(&dir, &["checkout", "-q", "-b", "clean", &first]);
    fs::write(dir.join("clean.txt"), "clean\n").unwrap();
    fixture(&dir, &["add", "clean.txt"]);
    fixture(&dir, &["commit", "-q", "-m", "clean change"]);
    fixture(&dir, &["checkout", "-q", "main"]);
    fixture(&dir, &["reset", "-q", "--hard", &first]);
    let git = repo(&dir);

    let merged = git.merge_no_ff("Stack: merge clean", "clean").unwrap();
    assert_eq!(merged.returncode, 0, "{}", merged.stderr);
    assert_eq!(
        merged.args,
        [
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            "Stack: merge clean",
            "clean"
        ]
    );
    let log = fixture(&dir, &["log", "-1", "--format=%an <%ae>|%cn|%P|%s"]);
    let parts: Vec<&str> = log.split('|').collect();
    assert_eq!(
        parts[0],
        "Workgraph Stack Integration <integration@workgraph.local>"
    );
    assert_eq!(parts[1], "Workgraph Stack Integration");
    assert_eq!(
        parts[2].split(' ').count(),
        2,
        "a merge commit has two parents"
    );
    assert_eq!(parts[3], "Stack: merge clean");
}

#[test]
fn merge_ff_only_moves_a_branch_that_is_ahead() {
    let dir = scratch("merge-ff");
    let first = repo_with_commit(&dir);
    fixture(&dir, &["checkout", "-q", "-b", "ahead"]);
    fs::write(dir.join("ahead.txt"), "x\n").unwrap();
    fixture(&dir, &["add", "ahead.txt"]);
    fixture(&dir, &["commit", "-q", "-m", "ahead"]);
    let ahead = fixture(&dir, &["rev-parse", "HEAD"]);
    fixture(&dir, &["checkout", "-q", "main"]);
    let git = repo(&dir);

    git.merge_ff_only(&ahead, RunOptions::CHECKED).unwrap();
    assert_eq!(git.head().unwrap(), Some(ahead.clone()));
    assert!(
        git.merge_ff_only(&first, RunOptions::CHECKED).is_ok(),
        "already contains"
    );
}

#[test]
fn update_ref_is_a_compare_and_swap() {
    let dir = scratch("cas");
    let first = repo_with_commit(&dir);
    fs::write(dir.join("file.txt"), "two\n").unwrap();
    fixture(&dir, &["commit", "-q", "-am", "second"]);
    let second = fixture(&dir, &["rev-parse", "HEAD"]);
    let git = repo(&dir);

    // main is at `second`: moving it while expecting `first` must fail and leave it alone
    let refused = git.update_ref("refs/heads/main", &first, &first, RunOptions::CHECKED);
    assert!(
        matches!(refused, Err(GitError::Failed { .. })),
        "{refused:?}"
    );
    assert_eq!(git.resolve_commit("main").unwrap(), second);

    git.update_ref("refs/heads/main", &first, &second, RunOptions::CHECKED)
        .unwrap();
    assert_eq!(git.resolve_commit("main").unwrap(), first);
}

#[test]
fn worktree_add_and_remove_round_trip() {
    let dir = scratch("worktree");
    let (first, _) = diverged_repo(&dir);
    fixture(&dir, &["branch", "wt-base", &first]);
    let git = repo(&dir);
    let existing = scratch("worktree-parent").join("existing");
    let created = scratch("worktree-parent").join("created");
    let detached = scratch("worktree-parent").join("detached");
    let (existing_s, created_s, detached_s) = (
        existing.display().to_string(),
        created.display().to_string(),
        detached.display().to_string(),
    );

    git.worktree_add_existing(&existing_s, "feature").unwrap();
    git.worktree_add_branch("wt-new", &created_s, &first)
        .unwrap();
    git.worktree_add_detached(&detached_s, &first).unwrap();
    let entries = git.worktrees().unwrap();
    assert_eq!(entries.len(), 4, "main plus three linked worktrees");
    assert!(
        Git {
            path: existing.clone(),
            timeout_ms: 120_000
        }
        .is_worktree()
        .unwrap()
    );

    // adding an existing branch a second time is refused
    let again = git.worktree_add_existing(&created_s, "feature");
    assert!(matches!(again, Err(GitError::Failed { .. })));

    // a clean worktree is removed without force; the detached one is removed with force
    git.worktree_remove(&existing_s, false, RunOptions::CHECKED)
        .unwrap();
    assert!(!existing.exists());
    fs::write(detached.join("dirty.txt"), "x").unwrap();
    let refused = git
        .worktree_remove(&detached_s, false, RunOptions::UNCHECKED)
        .unwrap();
    assert_ne!(refused.returncode, 0, "a dirty worktree needs force");
    let forced = git
        .worktree_remove(&detached_s, true, RunOptions::CHECKED)
        .unwrap();
    assert_eq!(
        forced.args,
        ["worktree", "remove", "--force", detached_s.as_str()]
    );
    git.worktree_prune(RunOptions::CHECKED).unwrap();
    assert_eq!(git.worktrees().unwrap().len(), 2);
}

#[test]
fn reset_clean_checkout_and_branch_delete() {
    let dir = scratch("reset");
    let (first, _) = diverged_repo(&dir);
    let git = repo(&dir);

    fs::write(dir.join("loose.txt"), "x").unwrap();
    git.clean_force(RunOptions::CHECKED).unwrap();
    assert!(!dir.join("loose.txt").exists());

    git.reset_hard(&first, RunOptions::CHECKED).unwrap();
    assert_eq!(git.head().unwrap(), Some(first.clone()));
    git.reset_keep(&first, RunOptions::CHECKED).unwrap();

    git.checkout_detach_force(&first, RunOptions::CHECKED)
        .unwrap();
    assert_eq!(git.current_branch().unwrap(), None);

    git.branch_delete("feature", RunOptions::CHECKED).unwrap();
    assert!(
        !git.local_branches()
            .unwrap()
            .contains(&"feature".to_owned())
    );
}

#[test]
fn a_call_past_its_timeout_is_a_run_error() {
    let dir = scratch("timeout");
    repo_with_commit(&dir);
    let git = Git {
        path: dir,
        timeout_ms: 1,
    };
    match git.run(&["status", "--porcelain"]) {
        Err(GitError::Run(message)) => {
            assert!(
                message.starts_with("git status --porcelain timed out after "),
                "{message}"
            );
        }
        other => panic!("expected a timeout, got {other:?}"),
    }
}
