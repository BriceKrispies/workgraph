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

// covers: E24
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

    let refs = git.refs().unwrap();
    assert_eq!(refs.get("refs/heads/main"), Some(&first));
    assert_eq!(
        refs.get("refs/tags/v1"),
        Some(&first),
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
