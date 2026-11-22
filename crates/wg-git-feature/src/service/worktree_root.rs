//! Shell: `execution.worktree_root`, a configured directory that holds attempt worktrees as
//! `<root>/<task-key>-<n>` instead of `<data>/worktrees/<attempt id>`. Short paths keep deep
//! repositories under Windows' MAX_PATH. The root may also hold the user's own worktrees, so only
//! recorded attempt directories directly inside it are ever treated as Workgraph's.

use std::path::PathBuf;

pub use crate::spec::worktree_root::in_attempt_dir;
use serde_json::{Map, Value};
use wg_runtime::App;
use wg_types::{Id, WgError, WorktreeStatus};

/// The `execution.worktree_root` of an effective configuration, `~` expanded.
#[must_use]
pub fn root_of(config: &Map<String, Value>) -> Option<PathBuf> {
    let text = config
        .get("execution")?
        .get("worktree_root")?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())?;
    Some(expand_home(text))
}

/// The configured root of the workspace `workspace_id`, if it or the global config sets one.
pub fn workspace_root(app: &mut App, workspace_id: Id) -> Result<Option<PathBuf>, WgError> {
    let slug = app
        .runtime
        .store
        .read(|session| Ok(session.workspace_by_id(workspace_id)?.map(|ws| ws.slug)))?;
    Ok(root_of(&app.effective_config(slug.as_deref())?))
}

/// Every configured root: the global one and each workspace's.
pub fn configured_roots(app: &mut App) -> Result<Vec<PathBuf>, WgError> {
    let slugs: Vec<String> = app.runtime.store.read(|session| {
        Ok(session
            .workspaces()?
            .into_iter()
            .map(|ws| ws.slug)
            .collect())
    })?;
    let mut roots = Vec::new();
    for slug in std::iter::once(None).chain(slugs.iter().map(|slug| Some(slug.as_str()))) {
        if let Some(root) = root_of(&app.effective_config(slug)?)
            && !roots.contains(&root)
        {
            roots.push(root);
        }
    }
    Ok(roots)
}

/// The paths of the managed worktrees not yet removed: checkout discovery and repository
/// registration treat them as Workgraph's wherever they are.
pub fn live_managed_paths(app: &mut App) -> Result<Vec<String>, WgError> {
    app.runtime.store.read(|session| {
        Ok(session
            .worktrees_with_repository(None, None)?
            .into_iter()
            .filter(|row| row.worktree.status != WorktreeStatus::Removed)
            .map(|row| row.worktree.path)
            .collect())
    })
}

fn expand_home(text: &str) -> PathBuf {
    let rest = match text.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => rest,
        _ => return PathBuf::from(text),
    };
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    match home {
        Some(home) => PathBuf::from(home).join(rest.trim_start_matches(['/', '\\'])),
        None => PathBuf::from(text),
    }
}
