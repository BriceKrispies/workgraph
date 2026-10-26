//! The route table constructors of the task feature's operations: a `RouteSpec` names its method,
//! path, status, query parameters and body, and the bodies name OpenAPI components. Pure data.

use wg_serverspec::groups::{BodyRef, RouteSpec};

/// A body that must be present; `strip` as in [`BodyRef`].
pub const fn required(component: &'static str, strip: bool) -> Option<BodyRef> {
    Some(BodyRef {
        component,
        required: true,
        strip,
    })
}

/// A body that may be absent (`Model | None = None`).
pub const fn optional(component: &'static str, strip: bool) -> Option<BodyRef> {
    Some(BodyRef {
        component,
        required: false,
        strip,
    })
}

/// A route of the tasks group: a path under `/api/v1/ws/{ws}`.
pub const fn task_route(
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
