//! The task feature's routes (docs/rust-rewrite.md 4.16): the task reads, writes and lifecycle
//! group (IF-44, H023 to H032 and H036 to H046) and the acceptance group (IF-24, H033 to H035).
//!
//! [`TASK_SPECS`] and [`ACCEPTANCE_SPECS`] are the tables of operations; the server binds each
//! operation to its group's `run`, which reads the validated request and calls the feature's
//! [`crate::service`] entry point.

use wg_json::Value;
use wg_runtime::{App, RouteGroup, RouteRequest};
use wg_serverspec::groups::RouteSpec;
use wg_types::{ErrorCode, WgError};

use crate::service;
use crate::spec::acceptance::body_parts;
use crate::spec::query::{addressed, filter_body, first_shown, flag, graph_request, number, text};
use crate::spec::routes::{optional, required, task_route};

/// The routes of the task reads, writes and lifecycle group, in registration order.
pub const GROUP: RouteGroup = RouteGroup {
    specs: &TASK_SPECS,
    run: run_tasks,
};

/// The routes of the acceptance group, in registration order.
pub const ACCEPTANCE: RouteGroup = RouteGroup {
    specs: &ACCEPTANCE_SPECS,
    run: run_acceptance,
};

/// The task reads, writes and lifecycle group (IF-44): H023 to H046 without the acceptance
/// routes H033 to H035, which belong to IF-24.
pub const TASK_SPECS: [RouteSpec; 21] = [
    task_route(
        "H023",
        "GET",
        "/api/v1/ws/{ws}/tasks",
        200,
        &[
            "status",
            "priority",
            "project",
            "repository",
            "tag",
            "category",
            "assignee",
            "parent",
            "roots",
            "q",
            "created_after",
            "created_before",
            "archived",
            "sort",
            "descending",
            "limit",
            "offset",
        ],
        None,
    ),
    task_route(
        "H024",
        "POST",
        "/api/v1/ws/{ws}/tasks",
        201,
        &[],
        required("TaskCreate", true),
    ),
    task_route(
        "H025",
        "POST",
        "/api/v1/ws/{ws}/tasks/bulk",
        200,
        &[],
        required("BulkUpdate", true),
    ),
    task_route(
        "H026",
        "GET",
        "/api/v1/ws/{ws}/ready",
        200,
        &["project", "repository", "limit"],
        None,
    ),
    task_route(
        "H027",
        "GET",
        "/api/v1/ws/{ws}/graph",
        200,
        &["project", "repository", "root", "include_closed"],
        None,
    ),
    task_route("H028", "GET", "/api/v1/ws/{ws}/tasks/{ref}", 200, &[], None),
    task_route(
        "H029",
        "PATCH",
        "/api/v1/ws/{ws}/tasks/{ref}",
        200,
        &[],
        required("TaskUpdate", true),
    ),
    task_route(
        "H030",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/claim",
        200,
        &[],
        optional("ClaimRequest", true),
    ),
    task_route(
        "H031",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/release",
        200,
        &[],
        optional("ReasonBody", false),
    ),
    task_route(
        "H032",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/complete",
        200,
        &[],
        optional("CompleteBody", false),
    ),
    task_route(
        "H036",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/comments",
        201,
        &[],
        required("CommentCreate", true),
    ),
    task_route(
        "H037",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/progress",
        200,
        &[],
        required("ProgressReport", true),
    ),
    task_route(
        "H038",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/result",
        200,
        &[],
        required("ResultReport", true),
    ),
    task_route(
        "H039",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/dependencies",
        200,
        &[],
        required("DependencyBody", false),
    ),
    task_route(
        "H040",
        "DELETE",
        "/api/v1/ws/{ws}/tasks/{ref}/dependencies/{depends_on}",
        200,
        &[],
        None,
    ),
    task_route(
        "H041",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/relations",
        200,
        &[],
        required("RelationCreate", true),
    ),
    task_route(
        "H042",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/repositories",
        200,
        &[],
        required("RepoBody", false),
    ),
    task_route(
        "H043",
        "DELETE",
        "/api/v1/ws/{ws}/tasks/{ref}/repositories/{repository}",
        200,
        &[],
        None,
    ),
    task_route(
        "H044",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/archive",
        200,
        &["undo"],
        None,
    ),
    task_route(
        "H045",
        "GET",
        "/api/v1/ws/{ws}/tasks/{ref}/history",
        200,
        &["limit"],
        None,
    ),
    task_route(
        "H046",
        "GET",
        "/api/v1/ws/{ws}/tasks/{ref}/attempts",
        200,
        &["limit", "offset"],
        None,
    ),
];

/// The acceptance group (IF-24): accept (H033), reject (H034) and the list of decisions (H035).
pub const ACCEPTANCE_SPECS: [RouteSpec; 3] = [
    task_route(
        "H033",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/accept",
        200,
        &[],
        required("AcceptBody", false),
    ),
    task_route(
        "H034",
        "POST",
        "/api/v1/ws/{ws}/tasks/{ref}/reject",
        200,
        &[],
        required("RejectBody", false),
    ),
    task_route(
        "H035",
        "GET",
        "/api/v1/ws/{ws}/tasks/{ref}/acceptances",
        200,
        &[],
        None,
    ),
];

/// One operation on the application context: the request in, the document out. The permission
/// checks are the application's, made before anything is looked up as the Python services do.
fn run_tasks(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let RouteRequest {
        params,
        given,
        context,
        ..
    } = request;
    let (ws, reference) = addressed(&params);
    let ws = Some(ws);
    match id {
        "H023" => service::task_list(app, &context, ws, &filter_body(&params)),
        "H024" => service::task_create(app, &context, ws, &given),
        "H025" => service::task_bulk_update(app, &context, ws, &given),
        "H026" => service::task_ready(
            app,
            &context,
            ws,
            text(&params, "project"),
            text(&params, "repository"),
            None,
            number(&params, "limit", 50),
        ),
        "H027" => service::task_graph(app, &context, ws, &graph_request(&params)),
        "H028" => first_shown(service::task_show(
            app,
            &context,
            ws,
            &[reference.to_owned()],
        )?),
        "H029" => service::task_update(app, &context, ws, reference, &given),
        "H030" => service::task_claim(app, &context, ws, reference, &given),
        "H031" => service::task_release(app, &context, ws, reference, &given),
        "H032" => service::task_complete(app, &context, ws, reference, &given),
        "H036" => service::task_comment(app, &context, ws, reference, &given),
        "H037" => service::task_progress(app, &context, ws, reference, &given),
        "H038" => service::task_result(app, &context, ws, reference, &given),
        "H039" => service::task_add_dependency(app, &context, ws, reference, &given),
        "H040" => service::task_remove_dependency(
            app,
            &context,
            ws,
            reference,
            text(&params, "depends_on").unwrap_or_default(),
        ),
        "H041" => service::task_add_relation(app, &context, ws, reference, &given),
        "H042" => service::task_link_repository(app, &context, ws, reference, &given),
        "H043" => service::task_unlink_repository(
            app,
            &context,
            ws,
            reference,
            text(&params, "repository").unwrap_or_default(),
        ),
        "H044" => {
            service::task_archive(app, &context, ws, reference, !flag(&params, "undo", false))
        }
        "H045" => service::task_history_limited(
            app,
            &context,
            ws,
            reference,
            &[],
            number(&params, "limit", 500),
        ),
        "H046" => service::task_attempts(
            app,
            &context,
            ws,
            reference,
            number(&params, "limit", 50),
            number(&params, "offset", 0),
        ),
        other => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("route {other} has no handler"),
        )),
    }
}

/// One operation on the application context: the request in, the document out.
fn run_acceptance(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let (ws, reference) = addressed(&request.params);
    let ws = Some(ws);
    let context = &request.context;
    let (manual, reason) = body_parts(&request.body);
    match id {
        "H033" if manual => {
            service::acceptance::task_accept_manually(app, context, ws, reference, &reason)
        }
        "H033" => service::acceptance::task_accept(app, context, ws, reference, &reason),
        "H034" => service::acceptance::task_reject(app, context, ws, reference, &reason),
        "H035" => service::acceptance::task_acceptances(app, context, ws, reference),
        other => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("route {other} has no handler"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tasks_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = TASK_SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids.len(), 21);
        assert!(!ids.contains(&"H033") && !ids.contains(&"H034") && !ids.contains(&"H035"));
        for spec in &TASK_SPECS {
            assert_eq!(wg_serverspec::operations::group_of(spec.id), "IF-44");
        }
    }

    #[test]
    fn every_task_body_loads_from_the_snapshot() {
        let openapi = wg_wire::contract::openapi().unwrap_or_default();
        for spec in &TASK_SPECS {
            if let Some(body) = &spec.body {
                assert!(
                    wg_serverspec::groups::body_spec(&openapi, body).is_some(),
                    "{}",
                    spec.id
                );
            }
        }
    }
}
