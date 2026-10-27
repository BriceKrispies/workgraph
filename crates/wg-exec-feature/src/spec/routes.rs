//! The route table of the exec feature's HTTP operations (IF-46): the attempt and run routes and
//! the scheduler overview. Pure data: a `RouteSpec` names its method, path, status, query
//! parameters and body, and the bodies name OpenAPI components.

use std::path::PathBuf;

use serde_json::{Map, Value};
use wg_serverspec::groups::{BodyRef, RouteSpec};

use crate::spec::runs::StartRequest;

/// The file name of an attempt's agent log under its run directory (`execute.rs` writes it).
const LOG_NAME: &str = "agent.log";

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

/// One operation of the exec feature's routes.
pub const fn exec_route(
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

/// The operations of the attempt, run and scheduler routes, in registration order: H071 to H079
/// and H122. Every body model here subclasses `BaseModel`, not `InputSchema`, so none is stripped.
pub const SPECS: [RouteSpec; 10] = [
    exec_route(
        "H071",
        "GET",
        "/api/v1/ws/{ws}/attempts",
        200,
        &["task", "status", "provider", "limit", "offset"],
        None,
    ),
    exec_route(
        "H072",
        "POST",
        "/api/v1/ws/{ws}/runs",
        201,
        &[],
        required("RunStart", false),
    ),
    exec_route(
        "H073",
        "GET",
        "/api/v1/attempts/compare",
        200,
        &["a", "b"],
        None,
    ),
    exec_route(
        "H074",
        "GET",
        "/api/v1/attempts/{attempt_id}",
        200,
        &["events"],
        None,
    ),
    exec_route(
        "H075",
        "GET",
        "/api/v1/attempts/{attempt_id}/manifests",
        200,
        &[],
        None,
    ),
    exec_route(
        "H076",
        "GET",
        "/api/v1/attempts/{attempt_id}/log",
        200,
        &["tail"],
        None,
    ),
    exec_route(
        "H077",
        "POST",
        "/api/v1/attempts/{attempt_id}/cancel",
        200,
        &[],
        optional("CancelBody", false),
    ),
    exec_route(
        "H078",
        "POST",
        "/api/v1/attempts/{attempt_id}/retry",
        200,
        &[],
        None,
    ),
    exec_route(
        "H079",
        "POST",
        "/api/v1/attempts/{attempt_id}/rerun",
        200,
        &[],
        None,
    ),
    exec_route("H122", "GET", "/api/v1/ws/{ws}/scheduler", 200, &[], None),
];

/// The string items of a list parameter (`status=ready&status=done`); unset reads as empty.
pub fn strings(params: &Map<String, Value>, name: &str) -> Vec<String> {
    params
        .get(name)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The `StartRequest` that `POST /ws/{ws}/runs` builds from its `RunStart` body.
pub fn start_request(body: &Value) -> StartRequest {
    let member = |name: &str| body.get(name).and_then(Value::as_str).map(str::to_owned);
    StartRequest {
        agent: member("agent"),
        model: member("model"),
        instructions: member("instructions"),
        ..StartRequest::default()
    }
}

/// Where the agent log of attempt `reference` in workspace `workspace_id` lives under the
/// artifacts directory `artifacts_dir` (`agent.log` in the attempt's `runs` directory).
pub fn log_path(artifacts_dir: &str, workspace_id: &str, reference: &str) -> PathBuf {
    PathBuf::from(artifacts_dir)
        .join(workspace_id)
        .join("runs")
        .join(reference)
        .join(LOG_NAME)
}

/// The index of the first kept line of an attempt log for Python's `lines[-tail:]` over `total`
/// lines: a positive tail keeps its last lines, a zero tail keeps every line and a negative one
/// drops its first lines.
pub fn tail_start(total: usize, tail: i64) -> usize {
    if tail > 0 {
        total.saturating_sub(usize::try_from(tail).unwrap_or(usize::MAX))
    } else {
        usize::try_from(tail.unsigned_abs()).map_or(total, |drop| drop.min(total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_follows_python_slicing() {
        assert_eq!(tail_start(10, 3), 7);
        assert_eq!(tail_start(10, 50), 0);
        assert_eq!(tail_start(10, 0), 0);
        assert_eq!(tail_start(10, -3), 3);
        assert_eq!(tail_start(2, -5), 2);
    }

    #[test]
    fn the_table_has_the_attempt_run_and_scheduler_operations() {
        let ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            [
                "H071", "H072", "H073", "H074", "H075", "H076", "H077", "H078", "H079", "H122"
            ]
        );
        for spec in &SPECS {
            assert_eq!(wg_serverspec::operations::group_of(spec.id), "IF-46");
        }
    }
}
