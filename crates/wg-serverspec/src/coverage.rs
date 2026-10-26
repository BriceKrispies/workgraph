//! The route-table check of plan 4.8 and 5.5: every route the server registers is in the OpenAPI
//! snapshot with the same method, path parameters, query parameters, body and success status;
//! every operation of a group listed as implemented is registered; the groups not yet
//! implemented are reported, never failed.

use std::collections::BTreeMap;

use serde_json::Value as Json;

use crate::operations::{GROUPS, OPERATIONS, group_of};
use crate::request::operation;
use crate::routes::Template;

/// What a registered route declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecl<'a> {
    pub id: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub status: u16,
    pub query: &'a [&'a str],
    /// `Some(required)` when the route takes a body.
    pub body: Option<bool>,
}

fn names(params: &[Json], location: &str) -> Vec<String> {
    params
        .iter()
        .filter(|param| param.get("in").and_then(Json::as_str) == Some(location))
        .filter_map(|param| param.get("name").and_then(Json::as_str).map(str::to_owned))
        .collect()
}

/// Every way `route` differs from the snapshot and the inventory; empty when it matches.
pub fn route_problems(openapi: &Json, route: &RouteDecl<'_>) -> Vec<String> {
    let label = format!("{} {} {}", route.id, route.method, route.path);
    let Some(op) = operation(openapi, route.method, route.path) else {
        return vec![format!("{label}: not in the OpenAPI snapshot")];
    };
    let mut problems = Vec::new();
    match OPERATIONS.iter().find(|known| known.id == route.id) {
        Some(known) if known.method == route.method && known.path == route.path => {
            if known.status != route.status {
                problems.push(format!(
                    "{label}: status {} but the inventory says {}",
                    route.status, known.status
                ));
            }
        }
        Some(known) => problems.push(format!(
            "{label}: the inventory has {} as {} {}",
            known.id, known.method, known.path
        )),
        None => problems.push(format!("{label}: unknown operation id")),
    }
    let responds = op
        .get("responses")
        .and_then(|responses| responses.get(route.status.to_string()))
        .is_some();
    if !responds {
        problems.push(format!(
            "{label}: the snapshot has no {} response",
            route.status
        ));
    }
    let params: Vec<Json> = op
        .get("parameters")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    let path_params = Template::parse(route.path).param_names();
    if path_params != names(&params, "path") {
        problems.push(format!(
            "{label}: path parameters {path_params:?}, snapshot {:?}",
            names(&params, "path")
        ));
    }
    let query: Vec<String> = route.query.iter().map(|q| (*q).to_owned()).collect();
    if query != names(&params, "query") {
        problems.push(format!(
            "{label}: query parameters {query:?}, snapshot {:?}",
            names(&params, "query")
        ));
    }
    let body = op
        .get("requestBody")
        .map(|body| body.get("required").and_then(Json::as_bool) == Some(true));
    if body != route.body {
        problems.push(format!(
            "{label}: body {:?} (Some(required)), snapshot {body:?}",
            route.body
        ));
    }
    problems
}

/// The coverage of the registered operation ids against the implemented groups.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Coverage {
    /// Operations of implemented groups that are not registered.
    pub missing: Vec<String>,
    /// Operations registered twice.
    pub duplicates: Vec<String>,
    /// Registered operations of groups not listed as implemented.
    pub unlisted: Vec<String>,
    /// Listed group names that are not groups.
    pub unknown_groups: Vec<String>,
    /// Groups not implemented yet, with their operation counts.
    pub pending: Vec<(String, usize)>,
}

impl Coverage {
    /// Whether anything fails (pending groups never do).
    pub fn failed(&self) -> bool {
        !(self.missing.is_empty()
            && self.duplicates.is_empty()
            && self.unlisted.is_empty()
            && self.unknown_groups.is_empty())
    }
}

/// Compares the registered operation ids with the groups listed as implemented.
pub fn coverage(registered: &[&str], implemented: &[&str]) -> Coverage {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for id in registered {
        *counts.entry(id).or_default() += 1;
    }
    let missing = OPERATIONS
        .iter()
        .filter(|op| implemented.contains(&group_of(op.id)) && !counts.contains_key(op.id))
        .map(|op| op.id.to_owned())
        .collect();
    let duplicates = counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(id, _)| (*id).to_owned())
        .collect();
    let unlisted = counts
        .keys()
        .filter(|id| !implemented.contains(&group_of(id)))
        .map(|id| (*id).to_owned())
        .collect();
    let unknown_groups = implemented
        .iter()
        .filter(|group| !GROUPS.contains(group))
        .map(|group| (*group).to_owned())
        .collect();
    let pending = GROUPS
        .iter()
        .filter(|group| !implemented.contains(group))
        .map(|group| {
            let count = OPERATIONS
                .iter()
                .filter(|op| group_of(op.id) == *group)
                .count();
            ((*group).to_owned(), count)
        })
        .collect();
    Coverage {
        missing,
        duplicates,
        unlisted,
        unknown_groups,
        pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matching_route_has_no_problems() {
        let doc = wg_wire::contract::openapi().unwrap();
        let events = RouteDecl {
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
        };
        assert_eq!(route_problems(&doc, &events), Vec::<String>::new());
        let create = RouteDecl {
            id: "H063",
            method: "POST",
            path: "/api/v1/ws/{ws}/stacks",
            status: 201,
            query: &[],
            body: Some(true),
        };
        assert_eq!(route_problems(&doc, &create), Vec::<String>::new());
    }

    #[test]
    fn differences_are_named() {
        let doc = wg_wire::contract::openapi().unwrap();
        let wrong = RouteDecl {
            id: "H063",
            method: "POST",
            path: "/api/v1/ws/{ws}/stacks",
            status: 200,
            query: &["x"],
            body: None,
        };
        assert_eq!(route_problems(&doc, &wrong).len(), 4);
        let absent = RouteDecl {
            path: "/api/v1/nope",
            ..wrong
        };
        assert_eq!(route_problems(&doc, &absent).len(), 1);
    }

    #[test]
    fn coverage_fails_only_for_listed_groups() {
        let none = coverage(&[], &[]);
        assert!(!none.failed());
        assert_eq!(none.pending.len(), GROUPS.len());
        let partial = coverage(&["H123"], &["IF-49"]);
        assert_eq!(partial.missing, vec!["H124"]);
        assert!(partial.failed());
        let unlisted = coverage(&["H001"], &[]);
        assert_eq!(unlisted.unlisted, vec!["H001"]);
        assert!(coverage(&[], &["IF-99"]).failed());
    }
}
