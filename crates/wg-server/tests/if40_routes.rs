//! The route-table test (plan 4.8, 5.5): every API route the server registers is in the OpenAPI
//! snapshot with the same method, path and query parameters, body and success status, and every
//! operation of each group in `wg_server::groups::IMPLEMENTED` is registered. Groups not listed
//! yet are reported on stderr, never failed.

use wg_server::groups::{IMPLEMENTED, routes};
use wg_serverspec::coverage::{RouteDecl, coverage, route_problems};

// covers: X1
#[test]
fn registered_routes_match_the_openapi_snapshot() {
    let openapi = wg_wire::contract::openapi().unwrap();
    let registered = routes();
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
    let report = coverage(&ids, IMPLEMENTED);
    for (group, count) in &report.pending {
        eprintln!("route group {group}: not implemented yet ({count} operations)");
    }
    assert!(!report.failed(), "{report:#?}");
}
