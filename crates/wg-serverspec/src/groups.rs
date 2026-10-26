//! The route groups as data (plan 9.9): which operations a group serves, with their method, path,
//! success status, query parameters and body component. The shell (`wg-server`) binds each entry to
//! its handler; the table itself is a pure value, so it lives here.

use serde_json::Value as Json;
use wg_validate::{LoadOptions, Schema, model_from_json_schema};

use crate::request::BodySpec;

/// The JSON body of an operation: the component of the OpenAPI snapshot it is read as, and whether
/// the body may be absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyRef {
    pub component: &'static str,
    pub required: bool,
}

/// One operation of a route group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteSpec {
    /// The inventory id, `H001` to `H130`.
    pub id: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    /// The success status (`status_code=` of the FastAPI route).
    pub status: u16,
    /// The query parameter names the route reads.
    pub query: &'static [&'static str],
    pub body: Option<BodyRef>,
}

/// The meta and settings group: health and meta (H001, H002), settings (H117, H118).
pub const META_SETTINGS: [RouteSpec; 4] = [
    RouteSpec {
        id: "H001",
        method: "GET",
        path: "/api/v1/health",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H002",
        method: "GET",
        path: "/api/v1/meta",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H117",
        method: "GET",
        path: "/api/v1/settings",
        status: 200,
        query: &["workspace"],
        body: None,
    },
    RouteSpec {
        id: "H118",
        method: "PATCH",
        path: "/api/v1/settings",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "SettingsPatch",
            required: true,
        }),
    },
];

/// The decisions group (`api/routes/decisions.py`): the list and one decision (H125, H126), the
/// decisions of a task (H129, H130) and the answers (H127, H128), in registration order.
pub const DECISIONS: [RouteSpec; 6] = [
    RouteSpec {
        id: "H125",
        method: "GET",
        path: "/api/v1/ws/{ws}/decisions",
        status: 200,
        query: &["status", "limit"],
        body: None,
    },
    RouteSpec {
        id: "H126",
        method: "GET",
        path: "/api/v1/ws/{ws}/decisions/{decision_id}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H129",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/decisions",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H130",
        method: "POST",
        path: "/api/v1/ws/{ws}/tasks/{ref}/decisions",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "DecisionRequest",
            required: true,
        }),
    },
    RouteSpec {
        id: "H127",
        method: "POST",
        path: "/api/v1/ws/{ws}/decisions/{decision_id}/choose",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "DecisionChoose",
            required: true,
        }),
    },
    RouteSpec {
        id: "H128",
        method: "POST",
        path: "/api/v1/ws/{ws}/decisions/{decision_id}/withdraw",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "DecisionWithdraw",
            required: false,
        }),
    },
];

/// The events group (IF-49): the history (H123) and the server-sent stream (H124).
pub const EVENTS: [RouteSpec; 2] = [
    RouteSpec {
        id: "H123",
        method: "GET",
        path: "/api/v1/ws/{ws}/events",
        status: 200,
        query: &[
            "after_seq",
            "entity_type",
            "entity_id",
            "prefix",
            "limit",
            "descending",
        ],
        body: None,
    },
    RouteSpec {
        id: "H124",
        method: "GET",
        path: "/api/v1/ws/{ws}/events/stream",
        status: 200,
        query: &["after_seq", "max_events"],
        body: None,
    },
];

/// The request body a [`BodyRef`] names, read from the OpenAPI document: the component's object
/// model, which the route validates bodies against. `None` when the component is missing.
pub fn body_spec(openapi: &Json, body: &BodyRef) -> Option<BodySpec> {
    let reference = format!("#/components/schemas/{}", body.component);
    let model = model_from_json_schema(
        openapi,
        &serde_json::json!({"$ref": reference}),
        LoadOptions::default(),
    )
    .ok()?;
    Some(BodySpec {
        schema: Schema::Model(Box::new(model)),
        required: body.required,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meta_settings_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = META_SETTINGS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids, ["H001", "H002", "H117", "H118"]);
        for spec in &META_SETTINGS {
            assert_eq!(crate::operations::group_of(spec.id), "meta-settings");
        }
    }

    #[test]
    fn the_decisions_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = DECISIONS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids, ["H125", "H126", "H129", "H130", "H127", "H128"]);
        for spec in &DECISIONS {
            assert_eq!(crate::operations::group_of(spec.id), "decisions");
        }
    }

    #[test]
    fn the_decision_bodies_load_from_the_snapshot() {
        let openapi = wg_wire::contract::openapi().unwrap_or_default();
        for spec in &DECISIONS {
            if let Some(body) = &spec.body {
                assert!(body_spec(&openapi, body).is_some(), "{}", spec.id);
            }
        }
    }

    #[test]
    fn the_events_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = EVENTS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids, ["H123", "H124"]);
        for spec in &EVENTS {
            assert_eq!(crate::operations::group_of(spec.id), "IF-49");
        }
    }

    #[test]
    fn the_settings_patch_body_loads_from_the_snapshot() {
        let openapi = wg_wire::contract::openapi().unwrap_or_default();
        let body = META_SETTINGS[3].body.unwrap_or(BodyRef {
            component: "",
            required: false,
        });
        assert!(body_spec(&openapi, &body).is_some());
    }
}
