//! The git feature's routes (docs/rust-rewrite.md 4.16): the worktree listing `GET
//! /api/v1/ws/{ws}/worktrees` (H022, IF-43) from `api/routes/core.py`, and the task commit and
//! change routes (H047 to H051, IF-45) from `api/routes/tasks.py`: the linked commits, the explicit
//! link, the cumulative changes, the live diff and the change sets.
//!
//! [`SPECS`] is the table of operations; the server binds it to [`run`], which reads the validated
//! request and calls [`crate::service`]. The repository routes that run git (`sync`, `commits`,
//! `commits/{sha}`) are the registry feature's, and call into this crate's services.

use serde_json::Value as Json;
use wg_iface::http::groups::{BodyRef, RouteSpec};
use wg_json::Value;
use wg_runtime::spec::text_param;
use wg_runtime::{App, RouteAnswer, RouteGroup, RouteRequest};
use wg_types::WgError;
use wg_types::pyval::int_or;

use crate::service;
use crate::spec::reads::float_param;

/// The `min_confidence` of `GET /tasks/{ref}/commits` when the client sends none.
const COMMITS_MIN_CONFIDENCE: f64 = 0.0;

/// The `min_confidence` of `GET /tasks/{ref}/changes` when the client sends none.
const CHANGES_MIN_CONFIDENCE: f64 = 0.5;

/// The `max_bytes` of `GET /tasks/{ref}/diff` when the client sends none.
const DIFF_MAX_BYTES: i64 = 400_000;

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 6] = [
    RouteSpec {
        id: "H022",
        method: "GET",
        path: "/api/v1/ws/{ws}/worktrees",
        status: 200,
        query: &["status"],
        body: None,
    },
    RouteSpec {
        id: "H047",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/commits",
        status: 200,
        query: &["min_confidence"],
        body: None,
    },
    RouteSpec {
        id: "H048",
        method: "POST",
        path: "/api/v1/ws/{ws}/tasks/{ref}/commits",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "LinkCommitBody",
            strip: false,
            required: true,
        }),
    },
    RouteSpec {
        id: "H049",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/changes",
        status: 200,
        query: &["min_confidence"],
        body: None,
    },
    RouteSpec {
        id: "H050",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/diff",
        status: 200,
        query: &["repository", "max_bytes"],
        body: None,
    },
    RouteSpec {
        id: "H051",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/change-sets",
        status: 200,
        query: &[],
        body: None,
    },
];

/// The group as the feature descriptor takes it.
pub const GROUP: RouteGroup = RouteGroup {
    specs: &SPECS,
    run: |app, id, request| run(app, id, request).map(RouteAnswer::Json),
};

/// One operation on the application context: the request in, the document out.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let context = &request.context;
    let params = &request.params;
    let ws = Some(text_param(params, "ws"));
    let task = text_param(params, "ref");
    match id {
        "H022" => service::git_worktrees(
            app,
            context,
            ws,
            params.get("status").and_then(Json::as_str),
        ),
        "H047" => service::task_commits(
            app,
            context,
            ws,
            task,
            float_param(params, "min_confidence", COMMITS_MIN_CONFIDENCE),
        ),
        "H048" => {
            let body = &request.body;
            let member = |name: &str| body.get(name).and_then(Json::as_str);
            service::git_link(
                app,
                context,
                ws,
                task,
                member("sha").unwrap_or_default(),
                member("repository"),
                member("note").unwrap_or_default(),
            )
        }
        "H049" => service::git_changes(
            app,
            context,
            ws,
            task,
            float_param(params, "min_confidence", CHANGES_MIN_CONFIDENCE),
        ),
        "H050" => {
            let max_bytes = usize::try_from(int_or(params.get("max_bytes"), DIFF_MAX_BYTES).max(0))
                .unwrap_or(usize::MAX);
            service::git_diff(
                app,
                context,
                ws,
                task,
                params.get("repository").and_then(Json::as_str),
                max_bytes,
            )
        }
        "H051" => service::change_sets(app, context, ws, task),
        other => Err(WgError::workgraph(format!("route {other} has no handler"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // covers: H022
    #[test]
    fn the_table_has_the_worktree_listing_at_its_path() {
        let spec = SPECS
            .iter()
            .find(|spec| spec.id == "H022")
            .expect("H022 is in the table");
        assert_eq!(spec.method, "GET");
        assert_eq!(spec.path, "/api/v1/ws/{ws}/worktrees");
        assert_eq!(spec.status, 200);
        assert!(spec.body.is_none());
        assert_eq!(wg_iface::http::operations::group_of(spec.id), "IF-43");
    }
}
