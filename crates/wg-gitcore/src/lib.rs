//! wg-gitcore: parsers for git output, attribution, rewrite reconciliation.
//!
//! Pure functions over the text `git` prints in its machine-readable (`-z`, porcelain) formats,
//! and over repository paths. No process is started here: `wg-git` runs the commands and hands
//! their output in. The golden vectors of GV-03 (`golden/gitparse/`) pin the results.
//!
//! Remote URL normalisation lives in `wg-registry` (`remote::normalize_remote_url`), where the
//! repository identity needs it; this crate does not repeat it.

mod classify;
mod diff;
mod error;
mod hook;
mod meta;
mod refs;
mod status;
mod worktree;

pub use classify::{file_class, language_for, module_for};
pub use diff::{CommitDiff, FileChange, parse_diff_tree, parse_numstat_z, parse_patch_ids};
pub use error::{GitError, HookError, RunOptions};
pub use hook::{HOOK_MARKER, HookPlan, plan_post_commit_hook, post_commit_script};
pub use meta::{CommitMeta, META_FORMAT, Trailer, cherry_picked_from, parse_meta, parse_trailers};
pub use refs::{
    choose_default_branch, failed_message, is_sha, is_valid_ref, non_empty_trimmed,
    parse_git_version, parse_refs, parse_remotes, parse_sha_lines, posix_path, split_words,
    stdin_lines, strip_verbatim_prefix, symref_branch,
};
pub use status::{WorkingTreeStatus, parse_status_porcelain_v2};
pub use worktree::{WorktreeEntry, parse_worktree_porcelain};
