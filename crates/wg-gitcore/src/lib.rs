//! wg-gitcore: parsers for git output, attribution, rewrite reconciliation.
//!
//! Pure functions over the text `git` prints in its machine-readable (`-z`, porcelain) formats,
//! and over repository paths. No process is started here: `wg-git` runs the commands and hands
//! their output in. The golden vectors of GV-03 (`golden/gitparse/`) pin the results.
//!
//! Remote URL normalisation lives in `wg-registry` (`remote::normalize_remote_url`), where the
//! repository identity needs it; this crate does not repeat it.

mod attribution;
mod classify;
mod diff;
mod meta;
mod worktree;

pub use attribution::{
    AMBIGUOUS_MESSAGE_CONFIDENCE, Candidate, CandidateSource, Evidence, MESSAGE_CONFIDENCE,
    MESSAGE_LABEL, TRAILER_CONFIDENCE, message_candidates, trailer_candidates,
};
pub use classify::{file_class, language_for, module_for};
pub use diff::{CommitDiff, FileChange, parse_diff_tree, parse_numstat_z, parse_patch_ids};
pub use meta::{CommitMeta, META_FORMAT, Trailer, cherry_picked_from, parse_meta, parse_trailers};
pub use worktree::{WorktreeEntry, parse_worktree_porcelain};
