//! The permissions an MCP agent acts with.
//!
//! `workgraph mcp --grant PERM` adds permissions to the defaults (`servers.py`:
//! `DEFAULT_AGENT_PERMISSIONS | parse_permissions(grant or [])`). The Python server applied the
//! grants to the stdio transport and to `mcp --transport http`, but `workgraph serve`'s `/mcp/`
//! mount got the defaults only (WG-3734). Both Rust transports take the set from this function.

use std::collections::BTreeSet;

use wg_types::{Permission, WgError, default_agent_permissions, parse_permissions};

/// The default agent permissions plus every permission in `grants`. An unknown name is a
/// `permission_denied` error, as in Python.
pub fn agent_permissions<S: AsRef<str>>(grants: &[S]) -> Result<BTreeSet<Permission>, WgError> {
    let mut permissions = default_agent_permissions();
    permissions.extend(parse_permissions(grants)?);
    Ok(permissions)
}

/// The permissions the Python method behind attempt, run, git and registry tool `name` requires,
/// or `None` for a tool this table does not cover.
pub fn tool_permissions(name: &str) -> Option<&'static [Permission]> {
    Some(match name {
        "workspace_list" | "workspace_get" | "project_list" | "project_get" => {
            &[Permission::WorkspaceRead]
        }
        "repository_list" | "repository_get" | "repository_status" => &[Permission::RepoRead],
        "repository_register" => &[Permission::RepoRegister],
        "attempt_create" => &[Permission::AttemptWrite],
        "attempt_start" => &[Permission::TaskClaim, Permission::AttemptWrite],
        "attempt_submit" => &[
            Permission::TaskClaim,
            Permission::AttemptWrite,
            Permission::VerifyRun,
        ],
        "attempt_get" | "attempt_list" | "run_get" => &[Permission::TaskRead],
        "run_start" => &[Permission::RunStart],
        "run_cancel" => &[Permission::RunCancel],
        "git_get_changes" | "git_get_task_diff" => &[Permission::GitRead],
        _ => return None,
    })
}

/// `require(ctx, *permissions)`: every needed permission must be granted to `actor`. The error
/// names the missing permissions, sorted, as `PermissionDenied` does in Python.
pub fn require(
    actor: &str,
    granted: &BTreeSet<Permission>,
    needed: &[Permission],
) -> Result<(), WgError> {
    let mut missing: Vec<&str> = needed
        .iter()
        .filter(|permission| !granted.contains(permission))
        .map(|permission| permission.as_str())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    missing.sort_unstable();
    Err(WgError::permission_denied(format!(
        "actor '{actor}' lacks permission(s): {}",
        missing.join(", ")
    ))
    .detail("actor", actor)
    .detail("missing", missing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_permissions_are_named_sorted() {
        let granted = BTreeSet::from([Permission::TaskRead]);
        let needed = [
            Permission::GitRead,
            Permission::TaskRead,
            Permission::AttemptWrite,
        ];
        let message = require("agent:x", &granted, &needed).map_err(|err| err.message);
        assert_eq!(
            message,
            Err("actor 'agent:x' lacks permission(s): attempt:write, git:read".to_owned())
        );
        assert!(require("agent:x", &granted, &[Permission::TaskRead]).is_ok());
    }

    #[test]
    fn grants_extend_the_defaults() {
        let none: [&str; 0] = [];
        let base = agent_permissions(&none).map(|p| p.len());
        let more = agent_permissions(&["run:start"]);
        assert_eq!(base.as_ref().ok(), Some(&default_agent_permissions().len()));
        assert!(more.is_ok_and(|p| p.contains(&Permission::RunStart)));
    }

    #[test]
    fn unknown_grant_is_refused() {
        assert!(agent_permissions(&["nope:nope"]).is_err());
    }
}
