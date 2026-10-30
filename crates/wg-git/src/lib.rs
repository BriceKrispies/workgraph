//! wg-git: git CLI runner and one function per git operation.
//!
//! The Rust counterpart of `integrations/git/cli.py` (the `Git` runner and its inspection
//! methods), of the git calls in `integrations/git/service.py`, and of `git install-hook`'s file
//! handling. Shell crate: every function here starts a process or touches the file system. Parsing
//! and decisions are `wg-gitcore`'s; the argument vectors, the E24 environment and the messages are
//! `wg-procspec`'s `effects` module, shared with the runtime so nothing is defined twice.

mod hook;
mod inspect;
mod ops;
mod runner;
mod stack;

pub use hook::{HookOutcome, install_post_commit_hook};
pub use inspect::resolve;
pub use runner::{Git, GitOutput, git_version};
pub use stack::INTEGRATION_IDENTITY;
pub use wg_gitcore::{GitError, HookError, RunOptions};
