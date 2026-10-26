//! Payloads of the `worktree.*` events (`integrations/git/worktrees.py` and the stack
//! integration and verification services that create managed worktrees).

use serde::Serialize;
use serde::ser::Serializer;
use serde_json::{Value, json};
use wg_types::Id;

/// `worktree.created`, written by three callers with three key sets:
///
/// * `prepare` (no `purpose`): `repository`, `path`, `branch` (null for a snapshot), `base_sha` and
///   `base` (the stack base's dict, or null);
/// * stack integration (`purpose`, `stack_id`): `branch` set, no `base`;
/// * stack bisect (`purpose`, `stack_id`): no `branch`, no `base`.
///
/// Serialization follows those shapes: `purpose` and `stack_id` only when set, `branch` unless a
/// stack caller left it unset, and `base` only for the `prepare` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCreated {
    pub purpose: Option<String>,
    pub stack_id: Option<Id>,
    pub repository: String,
    pub path: String,
    pub base_sha: String,
    pub base: Option<Value>,
    pub branch: Option<String>,
}

impl Serialize for WorktreeCreated {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let prepare_shape = self.purpose.is_none();
        let fields = self
            .purpose
            .iter()
            .map(|purpose| ("purpose", json!(purpose)))
            .chain(self.stack_id.iter().map(|stack| ("stack_id", json!(stack))))
            .chain([
                ("repository", json!(self.repository)),
                ("path", json!(self.path)),
            ])
            .chain((prepare_shape || self.branch.is_some()).then(|| ("branch", json!(self.branch))))
            .chain([("base_sha", json!(self.base_sha))])
            .chain(prepare_shape.then(|| ("base", json!(self.base))));
        serializer.collect_map(fields)
    }
}

/// `worktree.removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeRemoved {
    pub path: String,
    /// The branch of the removed worktree (`row.branch`); Python writes the name, not a flag.
    pub branch_kept: String,
}

/// `worktree.recovered`: how many worktree directories recovery touched, by outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorktreeRecovered {
    pub activated: i64,
    pub orphaned: i64,
    pub released: i64,
    pub unknown_dirs: i64,
}
