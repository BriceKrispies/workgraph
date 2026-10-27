//! The route-table test (plan 4.8, 5.5): every API route the server registers is in the OpenAPI
//! snapshot with the same method, path and query parameters, body and success status, and every
//! operation of each implemented group is registered. A group is implemented once one of its routes
//! is registered (derived from the registered routes, not kept by hand). Groups not implemented yet
//! are reported on stderr, never failed.

use wg_server::groups::routes_with;
use wg_serverspec::coverage::{RouteDecl, coverage, route_problems};

// covers: X1
#[test]
fn registered_routes_match_the_openapi_snapshot() {
    let openapi = wg_wire::contract::openapi().unwrap();
    let registered = routes_with(&[
        &wg_tasks_feature::FEATURE,
        &wg_decisions::FEATURE,
        &wg_registry_feature::FEATURE,
        &wg_telemetry_feature::FEATURE,
    ]);
    let problems: Vec<String> = registered
        .iter()
        .flat_map(|route| {
            route_problems(
                &openapi,
                &RouteDecl {
                    id: route.id,
                    method: route.method,
                    path: route.path,
                    status: route.status,
                    query: route.query,
                    body: route.body.as_ref().map(|body| body.required),
                },
            )
        })
        .collect();
    assert!(problems.is_empty(), "{}", problems.join("\n"));

    let ids: Vec<&str> = registered.iter().map(|route| route.id).collect();
    let mut implemented: Vec<&str> = registered
        .iter()
        .map(|route| wg_serverspec::operations::group_of(route.id))
        .collect();
    implemented.sort_unstable();
    implemented.dedup();
    let report = coverage(&ids, &implemented);
    for (group, count) in &report.pending {
        eprintln!("route group {group}: not implemented yet ({count} operations)");
    }
    assert!(!report.failed(), "{report:#?}");
}
