//! The verification, automation and notification routes (IF-47, H085 to H100): each operation is
//! registered by its owning feature with the method, path, query parameters, body and success
//! status of the OpenAPI snapshot, and the rule bodies the Python route takes as a plain dict are
//! read as open objects.

use serde_json::json;
use wg_server::groups::routes_with;
use wg_serverspec::coverage::{RouteDecl, route_problems};
use wg_serverspec::groups::{BodyRef, OPEN_OBJECT, body_spec};
use wg_serverspec::request::check_body;

/// The operation ids of IF-47 in order, H085 to H100.
fn if47_ids() -> Vec<String> {
    (85..=100).map(|number| format!("H{number:03}")).collect()
}

// covers: H085, H086, H087, H088, H089, H090, H091, H092, H093, H094, H095, H096, H097, H098, H099, H100
#[test]
fn the_verification_automation_and_notification_routes_match_the_snapshot() {
    let openapi = wg_wire::contract::openapi().unwrap();
    let registered = routes_with(&[&wg_verify_feature::FEATURE, &wg_automation_feature::FEATURE]);
    let group: Vec<&str> = registered
        .iter()
        .map(|route| route.id)
        .filter(|id| wg_serverspec::operations::group_of(id) == "IF-47")
        .collect();
    assert_eq!(group, if47_ids());

    let problems: Vec<String> = registered
        .iter()
        .filter(|route| wg_serverspec::operations::group_of(route.id) == "IF-47")
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
}

// covers: H093, H095, H096, H097
#[test]
fn the_rule_and_preview_bodies_are_read_from_the_snapshot() {
    let openapi = wg_wire::contract::openapi().unwrap();
    let registered = routes_with(&[&wg_automation_feature::FEATURE]);
    for id in ["H093", "H095", "H096", "H097"] {
        let route = registered.iter().find(|route| route.id == id).unwrap();
        assert!(route.body.is_some(), "{id} has no body");
    }
    // The dict bodies name no component: the open object is read inline.
    let open = BodyRef {
        component: OPEN_OBJECT,
        strip: false,
        required: true,
    };
    assert!(body_spec(&openapi, &open).is_some());
    let preview = BodyRef {
        component: "PreviewBody",
        strip: false,
        required: true,
    };
    assert!(body_spec(&openapi, &preview).is_some());
}

// covers: H085, H086, H087, H088, H089, H090, H091, H092, H094, H098, H099, H100
#[test]
fn the_reads_and_actions_without_a_body_register_without_one() {
    let registered = routes_with(&[&wg_verify_feature::FEATURE, &wg_automation_feature::FEATURE]);
    for id in [
        "H085", "H087", "H088", "H089", "H090", "H091", "H092", "H094", "H098", "H099", "H100",
    ] {
        let route = registered.iter().find(|route| route.id == id).unwrap();
        assert!(route.body.is_none(), "{id} should take no body");
    }
    let create = registered.iter().find(|route| route.id == "H086").unwrap();
    assert!(create.body.is_some(), "H086 takes a body");
}

// covers: H093, H095, H096
#[test]
fn a_dict_body_must_be_an_object_or_422() {
    let openapi = wg_wire::contract::openapi().unwrap();
    let open = BodyRef {
        component: OPEN_OBJECT,
        strip: false,
        required: true,
    };
    let spec = body_spec(&openapi, &open).unwrap();
    assert!(check_body(&spec, Some(&json!({"name": "rule", "trigger": {}}))).is_ok());
    let errors = check_body(&spec, Some(&json!(["not", "an", "object"]))).unwrap_err();
    assert_eq!(errors[0]["loc"], json!(["body"]));
    assert!(
        check_body(&spec, None).is_err(),
        "a required dict body is missing"
    );
}
