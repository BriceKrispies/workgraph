//! The `post-commit` hook that `git install-hook` writes (F13): its marker, its script and the
//! decision whether an existing hook may be replaced.

/// The first line after the shebang. A hook carrying it was written by Workgraph and may be
/// replaced; any other existing hook is left alone.
pub const HOOK_MARKER: &str = "# workgraph-managed hook";

/// What to do with the `post-commit` hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookPlan {
    /// Write this script (replacing a Workgraph-managed hook or none at all).
    Write(String),
    /// A hook exists that Workgraph did not write: refuse, do not overwrite.
    Foreign,
}

/// Decides the hook from the current content of `post-commit` (`None` when there is no file),
/// the posix form of the `workgraph` executable and the repository id.
pub fn plan_post_commit_hook(existing: Option<&str>, exe: &str, repository_id: &str) -> HookPlan {
    match existing {
        Some(text) if !text.contains(HOOK_MARKER) => HookPlan::Foreign,
        _ => HookPlan::Write(post_commit_script(exe, repository_id)),
    }
}

/// The script: a sync job for the repository, queued in the background and never failing the
/// commit.
pub fn post_commit_script(exe: &str, repository_id: &str) -> String {
    format!(
        "#!/bin/sh\n{HOOK_MARKER}\n\"{exe}\" job enqueue git_sync \
         --payload '{{\"repository_id\": \"{repository_id}\", \"mode\": \"incremental\"}}' \
         >/dev/null 2>&1 || true\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_when_absent_or_managed_and_refuses_foreign() {
        let plan = plan_post_commit_hook(None, "/bin/workgraph", "repo-1");
        let HookPlan::Write(script) = plan else {
            panic!("a missing hook is written");
        };
        assert!(script.starts_with(
            "#!/bin/sh\n# workgraph-managed hook\n\"/bin/workgraph\" job enqueue git_sync"
        ));
        assert!(
            script.contains(r#"--payload '{"repository_id": "repo-1", "mode": "incremental"}'"#)
        );
        assert!(script.ends_with(">/dev/null 2>&1 || true\n"));

        let managed = format!("#!/bin/sh\n{HOOK_MARKER}\nold\n");
        assert!(matches!(
            plan_post_commit_hook(Some(&managed), "w", "r"),
            HookPlan::Write(_)
        ));
        assert_eq!(
            plan_post_commit_hook(Some("#!/bin/sh\nmine\n"), "w", "r"),
            HookPlan::Foreign
        );
    }
}
