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
    /// Whether the model strips whitespace from its strings (`InputSchema` does, a plain
    /// pydantic model does not).
    pub strip: bool,
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

/// The meta group: health and meta (H001, H002). The settings routes (H117, H118) are the registry
/// feature's.
pub const META_SETTINGS: [RouteSpec; 2] = [
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
];

/// The stacks group (`api/routes/stacks.py`): the list and creation of stacks (H062, H063), one
/// stack (H064), its items (H065, H066), its history (H067), the stacks of a task (H068) and the
/// queued integration and verification (H069, H070), in registration order.
pub const STACKS: [RouteSpec; 9] = [
    RouteSpec {
        id: "H062",
        method: "GET",
        path: "/api/v1/ws/{ws}/stacks",
        status: 200,
        query: &["include_inactive"],
        body: None,
    },
    RouteSpec {
        id: "H063",
        method: "POST",
        path: "/api/v1/ws/{ws}/stacks",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "StackCreate",
            strip: true,
            required: true,
        }),
    },
    RouteSpec {
        id: "H064",
        method: "GET",
        path: "/api/v1/ws/{ws}/stacks/{ref}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H065",
        method: "POST",
        path: "/api/v1/ws/{ws}/stacks/{ref}/items",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "StackItemAdd",
            strip: true,
            required: true,
        }),
    },
    RouteSpec {
        id: "H066",
        method: "DELETE",
        path: "/api/v1/ws/{ws}/stacks/{ref}/items/{task}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H067",
        method: "GET",
        path: "/api/v1/ws/{ws}/stacks/{ref}/history",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H068",
        method: "GET",
        path: "/api/v1/ws/{ws}/tasks/{ref}/stacks",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H069",
        method: "POST",
        path: "/api/v1/ws/{ws}/stacks/{ref}/integrate",
        status: 202,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H070",
        method: "POST",
        path: "/api/v1/ws/{ws}/stacks/{ref}/verify",
        status: 202,
        query: &[],
        body: None,
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
        LoadOptions {
            strip_whitespace: body.strip,
        },
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
        assert_eq!(ids, ["H001", "H002"]);
        for spec in &META_SETTINGS {
            assert_eq!(crate::operations::group_of(spec.id), "meta-settings");
        }
    }

    #[test]
    fn the_stacks_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = STACKS.iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            [
                "H062", "H063", "H064", "H065", "H066", "H067", "H068", "H069", "H070"
            ]
        );
        for spec in &STACKS {
            assert_eq!(crate::operations::group_of(spec.id), "IF-53");
        }
    }

    #[test]
    fn the_stack_bodies_load_from_the_snapshot() {
        let openapi = wg_wire::contract::openapi().unwrap_or_default();
        for spec in &STACKS {
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
}
