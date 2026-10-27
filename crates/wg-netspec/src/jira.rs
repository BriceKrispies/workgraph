//! The read-only Jira REST client's pure parts (`integrations/trackers/jira/client.py`): the two
//! flavours (Cloud and Data Center), their search pagination, the request for each GET and the
//! reading of each response. Only GET is ever built here.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use wg_model::{HttpCall, HttpMethod};

use crate::error::TrackerError;
use crate::request::{Header, Query, call, url};
use crate::text::{percent_encode, py_repr};
use wg_types::pyval::{flag, prefix};

/// Issues per search page.
pub const PAGE_SIZE: u64 = 100;
/// The development-panel endpoint (undocumented; any error reads as no pull requests).
pub const DEV_STATUS_PATH: &str = "/rest/dev-status/latest/issue/detail";

/// What differs between Jira Cloud and Jira Data Center.
#[derive(Clone, PartialEq, Eq)]
pub enum Flavour {
    /// Jira Cloud: Basic auth with an account email and an API token; token pagination.
    Cloud { email: String, token: String },
    /// Jira Data Center: a bearer personal access token; offset pagination.
    DataCenter { token: String },
}

/// Where the next search page starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cursor {
    /// Cloud: the `nextPageToken` of the last page; `None` for the first page.
    Token(Option<String>),
    /// Data Center: the `startAt` offset.
    Offset(u64),
}

impl Flavour {
    /// The REST API prefix, e.g. `/rest/api/3`.
    pub fn api_path(&self) -> &'static str {
        match self {
            Self::Cloud { .. } => "/rest/api/3",
            Self::DataCenter { .. } => "/rest/api/2",
        }
    }

    /// The `fields` value that is enough to list `(id, key)` pairs.
    pub fn key_list_fields(&self) -> &'static str {
        match self {
            Self::Cloud { .. } => "id",
            Self::DataCenter { .. } => "key",
        }
    }

    /// The authorization header. Never log it or embed it in an error message.
    pub fn auth_headers(&self) -> Vec<Header> {
        match self {
            Self::Cloud { email, token } => {
                let raw = format!("{email}:{token}");
                vec![(
                    "Authorization".to_owned(),
                    format!("Basic {}", STANDARD.encode(raw.as_bytes())),
                )]
            }
            Self::DataCenter { token } => {
                vec![("Authorization".to_owned(), format!("Bearer {token}"))]
            }
        }
    }

    /// The search path of this flavour.
    pub fn search_path(&self) -> String {
        match self {
            Self::Cloud { .. } => format!("{}/search/jql", self.api_path()),
            Self::DataCenter { .. } => format!("{}/search", self.api_path()),
        }
    }

    /// The cursor of the first page.
    pub fn first_cursor(&self) -> Cursor {
        match self {
            Self::Cloud { .. } => Cursor::Token(None),
            Self::DataCenter { .. } => Cursor::Offset(0),
        }
    }

    /// The query parameters of the page at `cursor`.
    pub fn search_query(&self, jql: &str, fields: &str, cursor: &Cursor) -> Query {
        let mut query = vec![
            ("jql".to_owned(), jql.to_owned()),
            ("fields".to_owned(), fields.to_owned()),
        ];
        match cursor {
            Cursor::Token(Some(token)) => {
                query.push(("maxResults".to_owned(), PAGE_SIZE.to_string()));
                query.push(("nextPageToken".to_owned(), token.clone()));
            }
            Cursor::Token(None) => {
                query.push(("maxResults".to_owned(), PAGE_SIZE.to_string()));
            }
            Cursor::Offset(start) => {
                query.push(("startAt".to_owned(), start.to_string()));
                query.push(("maxResults".to_owned(), PAGE_SIZE.to_string()));
            }
        }
        query
    }

    /// The cursor of the page after the one read at `cursor`, or `None` when that page was the
    /// last. `page` is that page's body and `issue_count` the number of issues it held.
    pub fn next_cursor(&self, cursor: &Cursor, page: &Value, issue_count: usize) -> Option<Cursor> {
        match (self, cursor) {
            (Self::DataCenter { .. }, Cursor::Offset(start)) => {
                let next = start + issue_count as u64;
                let total = page.get("total").and_then(Value::as_u64).unwrap_or(0);
                if issue_count == 0 || next >= total {
                    None
                } else {
                    Some(Cursor::Offset(next))
                }
            }
            _ => {
                let token = page
                    .get("nextPageToken")
                    .and_then(Value::as_str)
                    .filter(|token| !token.is_empty());
                let last = flag(page.get("isLast"));
                match token {
                    Some(token) if !last => Some(Cursor::Token(Some(token.to_owned()))),
                    _ => None,
                }
            }
        }
    }
}

/// The `issues` of a search page; empty when there are none.
pub fn issues_of_page(page: &Value) -> Vec<Value> {
    page.get("issues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A GET of `path` with `query`, with the flavour's headers and `Accept: application/json`.
pub fn get_call(
    base_url: &str,
    flavour: &Flavour,
    path: &str,
    query: &[(String, String)],
    timeout_seconds: f64,
) -> HttpCall {
    let mut headers = vec![("Accept".to_owned(), "application/json".to_owned())];
    headers.extend(flavour.auth_headers());
    call(
        HttpMethod::Get,
        url(base_url, path, query),
        headers,
        None,
        timeout_seconds,
    )
}

/// The path of one issue, its key percent-encoded.
pub fn issue_path(flavour: &Flavour, key: &str) -> String {
    format!("{}/issue/{}", flavour.api_path(), percent_encode(key))
}

/// The query of the development-panel lookup for one issue id.
pub fn dev_status_query(external_id: &str) -> Query {
    vec![
        ("issueId".to_owned(), external_id.to_owned()),
        ("applicationType".to_owned(), "GitHub".to_owned()),
        ("dataType".to_owned(), "pullrequest".to_owned()),
    ]
}

/// The pull requests of a development-panel body: every `pullRequests` entry of every `detail`.
pub fn dev_pull_requests(body: &Value) -> Vec<Value> {
    body.get("detail")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|detail| detail.get("pullRequests").and_then(Value::as_array))
        .flatten()
        .cloned()
        .collect()
}

/// The `(id, key)` pairs of a page of issues listed with the key-list fields. An issue missing
/// either field is skipped.
pub fn key_pairs(issues: &[Value]) -> Vec<(String, String)> {
    issues
        .iter()
        .filter_map(|issue| {
            let id = scalar_text(issue.get("id")?);
            let key = scalar_text(issue.get("key")?);
            Some((id, key))
        })
        .collect()
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The user's time zone name from `/myself`, when it has a non-empty one.
pub fn time_zone_name(myself: &Value) -> Option<String> {
    myself
        .get("timeZone")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// The JSON object a GET answered with, or the error it stands for. A status outside 2xx is an
/// error carrying the status; the body is never quoted.
pub fn read_response(path: &str, status: u16, body: &str) -> Result<Value, TrackerError> {
    if !(200..300).contains(&status) {
        return Err(TrackerError {
            message: format!("Jira GET {path} returned HTTP {status}"),
            status: Some(status),
        });
    }
    match serde_json::from_str::<Value>(body) {
        Ok(object @ Value::Object(_)) => Ok(object),
        Ok(_) => Err(TrackerError {
            message: format!("Jira GET {path} returned a JSON value that is not an object"),
            status: None,
        }),
        Err(_) => Err(TrackerError {
            message: format!("Jira GET {path} returned invalid JSON"),
            status: None,
        }),
    }
}

/// A request that never got a response. `detail` is the transport's account of it.
pub fn transport_error(path: &str, detail: &str) -> TrackerError {
    TrackerError {
        message: format!("Jira GET {path} failed: {}", prefix(detail, 200)),
        status: None,
    }
}

/// The warning logged when the user's profile has no time zone.
pub const NO_TIME_ZONE_WARNING: &str = "Jira /myself has no timeZone; formatting JQL dates in UTC";

/// The warning logged when the user's time zone is one the host does not know.
pub fn unknown_time_zone_warning(name: &str) -> String {
    format!(
        "Jira user timeZone {} is unknown; formatting JQL dates in UTC",
        py_repr(name)
    )
}

/// The warning logged when a development-panel lookup fails.
pub fn dev_status_warning(external_id: &str, message: &str) -> String {
    format!("Jira dev-status lookup for issue {external_id} failed: {message}")
}
