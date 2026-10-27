//! The git feature's routes (docs/rust-rewrite.md 4.16): the worktree listing `GET
//! /api/v1/ws/{ws}/worktrees` (H022, IF-43) from `api/routes/core.py`.
//!
//! [`SPECS`] is the table of operations; the server binds it to [`run`], which reads the validated
//! request and calls [`crate::service::git_worktrees`]. The repository routes that run git
//! (`sync`, `commits`, `commits/{sha}`) are the registry feature's, and call into this crate's
//! services.

use serde_json::Value as Json;
use wg_json::Value;
use wg_runtime::{App, RouteGroup, RouteRequest};
use wg_runtimespec::text_param;
use wg_serverspec::groups::RouteSpec;
use wg_types::WgError;

use crate::service;

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 1] = [RouteSpec {
    id: "H022",
    method: "GET",
    path: "/api/v1/ws/{ws}/worktrees",
    status: 200,
    query: &["status"],
    body: None,
}];

/// The group as the feature descriptor takes it.
pub const GROUP: RouteGroup = RouteGroup { specs: &SPECS, run };

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    match id {
        "H022" => service::git_worktrees(
            app,
            &request.context,
            Some(text_param(&request.params, "ws")),
            request.params.get("status").and_then(Json::as_str),
        ),
        other => Err(WgError::workgraph(format!("route {other} has no handler"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // covers: H022
    #[test]
    fn the_table_has_the_worktree_listing_at_its_path() {
        assert_eq!(SPECS.len(), 1);
        let spec = &SPECS[0];
        assert_eq!(spec.id, "H022");
        assert_eq!(spec.method, "GET");
        assert_eq!(spec.path, "/api/v1/ws/{ws}/worktrees");
        assert_eq!(spec.status, 200);
        assert!(spec.body.is_none());
        assert_eq!(wg_serverspec::operations::group_of(spec.id), "IF-43");
    }
}
