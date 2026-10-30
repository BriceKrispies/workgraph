//! The jobs feature's routes (IF-46): H080 to H084, the job listing, one job, its cancel and
//! retry, and the workers. [`SPECS`] is the table of operations; [`GROUP`] binds each one to
//! [`run_jobs`], which reads the validated request and calls [`crate::service`].

use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_runtime::{App, RouteAnswer, RouteGroup, RouteRequest};
use wg_types::WgError;

use crate::service::{self, JobQuery};
use crate::spec::{SPECS, strings};
use wg_types::pyval::int_or;

/// The routes of the jobs feature, in registration order.
pub const GROUP: RouteGroup = RouteGroup {
    specs: &SPECS,
    run: |app, id, request| run_jobs(app, id, request).map(RouteAnswer::Json),
};

/// One operation on the application context: the request in, the document out.
fn run_jobs(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let RouteRequest {
        params, context, ..
    } = request;
    let job_id = text(&params, "job_id").unwrap_or_default();
    match id {
        "H080" => {
            let statuses = strings(&params, "status");
            service::list(
                app,
                &context,
                &JobQuery {
                    workspace: text(&params, "workspace"),
                    statuses: &statuses,
                    kind: text(&params, "kind"),
                    task: text(&params, "task"),
                    limit: number(&params, "limit", 100),
                    offset: number(&params, "offset", 0),
                },
            )
        }
        "H081" => service::get(app, &context, job_id),
        "H082" => service::cancel(app, &context, job_id),
        "H083" => service::retry(app, &context, job_id),
        "H084" => service::workers(app, &context),
        other => Err(WgError::workgraph(format!("route {other} has no handler"))),
    }
}

/// A string parameter of a query; `None` when unset.
fn text<'a>(params: &'a Map<String, Json>, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Json::as_str)
}

/// An integer parameter, `default` when unset.
fn number(params: &Map<String, Json>, name: &str, default: i64) -> i64 {
    int_or(params.get(name), default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_group_has_the_job_and_worker_operations_without_bodies() {
        assert_eq!(GROUP.specs.len(), 5);
        assert!(GROUP.specs.iter().all(|spec| spec.body.is_none()));
    }
}
