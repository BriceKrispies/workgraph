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
            strip: false,
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
            strip: true,
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
            strip: true,
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
            strip: true,
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

/// A route of the tasks group: a path under `/api/v1/ws/{ws}`.
const fn task_route(
    id: &'static str,
    method: &'static str,
    path: &'static str,
    status: u16,
    query: &'static [&'static str],
    body: Option<BodyRef>,
) -> RouteSpec {
    RouteSpec {
        id,
        method,
        path,
        status,
        query,
        body,
    }
}

/// A body that must be present; `strip` as in [`BodyRef`].
const fn required(component: &'static str, strip: bool) -> Option<BodyRef> {
    Some(BodyRef {
        component,
        required: true,
        strip,
    })
}

/// A body that may be absent (`Model | None = None`).
const fn optional(component: &'static str, strip: bool) -> Option<BodyRef> {
    Some(BodyRef {
        component,
        required: false,
        strip,
    })
}

/// The task reads, writes and lifecycle group (IF-44): H023 to H046 without the acceptance
/// routes H033 to H035, which belong to IF-24.
pub const TASKS: [RouteSpec; 21] = [
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
    fn the_tasks_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = TASKS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids.len(), 21);
        assert!(!ids.contains(&"H033") && !ids.contains(&"H034") && !ids.contains(&"H035"));
        for spec in &TASKS {
            assert_eq!(crate::operations::group_of(spec.id), "IF-44");
        }
    }

    #[test]
    fn every_task_body_loads_from_the_snapshot() {
        let openapi = wg_wire::contract::openapi().unwrap_or_default();
        for spec in &TASKS {
            if let Some(body) = &spec.body {
                assert!(body_spec(&openapi, body).is_some(), "{}", spec.id);
            }
        }
    }

    #[test]
    fn the_settings_patch_body_loads_from_the_snapshot() {
        let openapi = wg_wire::contract::openapi().unwrap_or_default();
        let body = META_SETTINGS[3].body.unwrap_or(BodyRef {
            component: "",
            required: false,
            strip: false,
        });
        assert!(body_spec(&openapi, &body).is_some());
    }
}
