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

#[cfg(test)]
mod tests {
    use super::*;

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
