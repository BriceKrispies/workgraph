//! `git install-hook` file handling (F13): the `post-commit` hook is read, decided and written
//! here; the decision and the script are `wg_gitcore`'s.

use std::path::{Path, PathBuf};

use wg_gitcore::{HookError, HookPlan, plan_post_commit_hook, posix_path};

use crate::runner::Git;

/// What `install-hook` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    /// The hook was written (or replaced a Workgraph-managed one) at this path.
    Installed(PathBuf),
    /// A hook Workgraph did not write exists at this path; it was left alone.
    Refused(PathBuf),
}

/// Installs `<git common dir>/hooks/post-commit`, which queues an incremental sync of
/// `repository_id` after each commit. An existing hook without the Workgraph marker is never
/// overwritten. The script names the `workgraph` executable found on `PATH`.
///
/// # Errors
/// [`HookError::Git`] when the git directory cannot be read, [`HookError::Io`] when the hook file
/// cannot be read or written.
pub fn install_post_commit_hook(git: &Git, repository_id: &str) -> Result<HookOutcome, HookError> {
    let hook = git
        .git_common_dir()
        .map_err(HookError::Git)?
        .join("hooks")
        .join("post-commit");
    let existing = match std::fs::read(&hook) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => return Err(HookError::Io(format!("{}: {source}", hook.display()))),
    };
    let exe = wg_proc::resolve_executable("workgraph")
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "workgraph".to_owned());
    let plan = plan_post_commit_hook(
        existing.as_deref(),
        &posix_path(&exe, cfg!(windows)),
        repository_id,
    );
    match plan {
        HookPlan::Foreign => Ok(HookOutcome::Refused(hook)),
        HookPlan::Write(script) => {
            std::fs::write(&hook, script)
                .map_err(|source| HookError::Io(format!("{}: {source}", hook.display())))?;
            make_executable(&hook)?;
            Ok(HookOutcome::Installed(hook))
        }
    }
}

/// Sets the execute bits (`0o755`) on unix. Windows has no execute bit to set.
fn make_executable(path: &Path) -> Result<(), HookError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .map_err(|source| HookError::Io(format!("{}: {source}", path.display())))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
