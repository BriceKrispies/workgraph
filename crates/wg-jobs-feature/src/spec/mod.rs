//! The jobs feature's pure half (Pure): the route table of its operations and the shaping of the
//! job listing's page and filters.

use serde_json::{Map, Value};
use wg_serverspec::groups::{BodyRef, RouteSpec};

/// One operation of the jobs routes. None of them takes a body.
const fn route(
    id: &'static str,
    method: &'static str,
    path: &'static str,
    query: &'static [&'static str],
) -> RouteSpec {
    RouteSpec {
        id,
        method,
        path,
        status: 200,
        query,
        body: None::<BodyRef>,
    }
}

/// The operations of the job and worker routes, in registration order.
pub const SPECS: [RouteSpec; 5] = [
    route(
        "H080",
        "GET",
        "/api/v1/jobs",
        &["workspace", "status", "kind", "task", "limit", "offset"],
    ),
    route("H081", "GET", "/api/v1/jobs/{job_id}", &[]),
    route("H082", "POST", "/api/v1/jobs/{job_id}/cancel", &[]),
    route("H083", "POST", "/api/v1/jobs/{job_id}/retry", &[]),
    route("H084", "GET", "/api/v1/workers", &[]),
];

/// The `(limit, offset)` of a job page as the queue's `LIMIT ... OFFSET` reads them: a negative
/// limit lists every job (SQLite's `LIMIT -1`), a negative offset starts at the first job.
pub fn page_window(limit: i64, offset: i64) -> (Option<usize>, usize) {
    (
        usize::try_from(limit).ok(),
        usize::try_from(offset).unwrap_or(0),
    )
}

/// The string items of a list parameter; unset reads as empty.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_the_job_and_worker_operations() {
        let ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids, ["H080", "H081", "H082", "H083", "H084"]);
        for spec in &SPECS {
            assert_eq!(wg_serverspec::operations::group_of(spec.id), "IF-46");
        }
    }

    #[test]
    fn a_negative_limit_lists_everything_and_a_negative_offset_starts_at_the_first_job() {
        assert_eq!(page_window(-1, -4), (None, 0));
        assert_eq!(page_window(100, 20), (Some(100), 20));
        assert_eq!(page_window(0, 0), (Some(0), 0));
    }
}
