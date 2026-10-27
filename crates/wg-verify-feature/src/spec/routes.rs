//! Pure: the verification routes (H085 to H090, `api/routes/ops.py`): the query parameters read
//! into the arguments of the services. The bodies and the path parameters are read by the route
//! table in [`crate::http`].

use serde_json::{Map, Value as Json};
use wg_types::{ErrorCode, WgError};

/// The `limit` of `GET /ws/{ws}/verification/runs` when the client sends none.
pub const RUNS_LIMIT: i64 = 50;

/// The `status` and `limit` of `GET /ws/{ws}/verification/runs`. An empty status is no filter,
/// as the Python service reads it.
/// The `action` of `POST /verification/configs/{config_id}/{action}`: `true` to enable, `false` to
/// disable. The snapshot's enum rejects any other word first; this is the same answer for a
/// route that reads it anyway.
pub fn enabled_of(action: &str) -> Result<bool, WgError> {
    match action {
        "enable" => Ok(true),
        "disable" => Ok(false),
        other => Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("unknown verification action {other}"),
        )),
    }
}

/// The `check`, `repository` and `since` of `GET /ws/{ws}/verification/failures`. An empty text is
/// no filter, as the Python service tests them (`if check_name:`).
#[must_use]
pub fn failure_args(params: &Map<String, Json>) -> (Option<&str>, Option<&str>, Option<&str>) {
    let text = |key: &str| {
        params
            .get(key)
            .and_then(Json::as_str)
            .filter(|found| !found.is_empty())
    };
    (text("check"), text("repository"), text("since"))
}

/// The `status` and `limit` of `GET /ws/{ws}/verification/runs`.
#[must_use]
pub fn run_args(params: &Map<String, Json>) -> (Option<&str>, i64) {
    let status = params
        .get("status")
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty());
    let limit = params
        .get("limit")
        .and_then(Json::as_i64)
        .unwrap_or(RUNS_LIMIT);
    (status, limit)
}
