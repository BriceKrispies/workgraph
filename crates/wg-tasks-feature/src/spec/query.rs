//! The query parameters of the task routes, read from the values the route layer validated.
//!
//! A parameter that was not sent is either missing from the map or `null`; both read as unset.

use serde_json::{Map, Value};
use wg_types::WgError;

use super::reads::{GraphRequest, Shown};

/// A string parameter; `None` when unset.
#[must_use]
pub fn text<'a>(params: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    params.get(name).and_then(Value::as_str)
}

/// The workspace and task reference a route addresses (`{ws}` and `{ref}`).
#[must_use]
pub fn addressed(params: &Map<String, Value>) -> (&str, &str) {
    (
        text(params, "ws").unwrap_or_default(),
        text(params, "ref").unwrap_or_default(),
    )
}

/// A boolean parameter, `default` when unset.
#[must_use]
pub fn flag(params: &Map<String, Value>, name: &str, default: bool) -> bool {
    params.get(name).and_then(Value::as_bool).unwrap_or(default)
}

/// An integer parameter, `default` when unset.
#[must_use]
pub fn number(params: &Map<String, Value>, name: &str, default: i64) -> i64 {
    params.get(name).and_then(Value::as_i64).unwrap_or(default)
}

/// The `TaskFilter` body that `GET /tasks` builds from its query (`list_tasks`): the query names
/// mapped to the filter fields, with the parameters that were not sent left out.
#[must_use]
pub fn filter_body(params: &Map<String, Value>) -> Value {
    const RENAMED: [(&str, &str); 12] = [
        ("status", "statuses"),
        ("priority", "priorities"),
        ("project", "project"),
        ("repository", "repository"),
        ("tag", "tags"),
        ("category", "category"),
        ("assignee", "assignee"),
        ("parent", "parent"),
        ("roots", "root_only"),
        ("q", "text"),
        ("created_after", "created_after"),
        ("created_before", "created_before"),
    ];
    const KEPT: [(&str, &str); 5] = [
        ("archived", "archived_only"),
        ("sort", "sort"),
        ("descending", "descending"),
        ("limit", "limit"),
        ("offset", "offset"),
    ];
    Value::Object(
        RENAMED
            .iter()
            .chain(KEPT.iter())
            .filter_map(|(from, to)| {
                params
                    .get(*from)
                    .filter(|value| !value.is_null())
                    .map(|value| ((*to).to_owned(), value.clone()))
            })
            .collect(),
    )
}

/// The `TaskService.graph` arguments of `GET /graph`.
#[must_use]
pub fn graph_request(params: &Map<String, Value>) -> GraphRequest<'_> {
    GraphRequest {
        root: text(params, "root"),
        project: text(params, "project"),
        repository: text(params, "repository"),
        include_closed: flag(params, "include_closed", true),
    }
}

/// `TaskService.detail` for one reference: the detail, or the error that reference failed with.
pub fn first_shown(shown: Shown) -> Result<wg_json::Value, WgError> {
    let Shown { items, errors } = shown;
    match (items.into_iter().next(), errors.into_iter().next()) {
        (Some(item), _) => Ok(item),
        (None, Some((_, error))) => Err(error),
        (None, None) => Ok(wg_json::Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn params(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn the_list_filter_renames_the_query_and_skips_what_was_not_sent() {
        let query = params(json!({
            "ws": "main",
            "status": ["ready"],
            "tag": ["a", "b"],
            "roots": true,
            "q": "text",
            "project": null,
            "archived": false,
            "limit": 5,
        }));
        assert_eq!(
            filter_body(&query),
            json!({
                "statuses": ["ready"],
                "tags": ["a", "b"],
                "root_only": true,
                "text": "text",
                "archived_only": false,
                "limit": 5,
            })
        );
    }

    #[test]
    fn unset_parameters_read_as_their_defaults() {
        let query = params(json!({"ws": "main", "ref": "WG-1", "root": null}));
        assert_eq!(addressed(&query), ("main", "WG-1"));
        assert!(flag(&query, "include_closed", true));
        assert_eq!(number(&query, "limit", 50), 50);
        assert_eq!(graph_request(&query).root, None);
    }
}
