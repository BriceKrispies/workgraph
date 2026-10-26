//! Jira issue JSON to a [`TrackerSnapshot`]: the fields Workgraph takes from one issue.

use std::collections::BTreeSet;
use std::fmt;

use serde_json::Value;
use wg_types::{TaskPriority, Timestamp};

use crate::adf::adf_to_text;
use crate::pull_request::{
    PullRequestRef, SOURCE_CUSTOM_FIELD, SOURCE_DEV_PANEL, merge, parse_pull_request_url, urls_in,
};
use crate::sync::project_of;

/// The mapping settings of one Jira connection that the mapper reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingConfig {
    /// Site base URL; trailing slashes are dropped before `/browse/<key>` is appended.
    pub base_url: String,
    /// Custom fields whose text may name pull requests.
    pub pr_fields: Vec<String>,
    /// Whether development-panel pull requests are read.
    pub dev_panel: bool,
}

/// The Jira status category, as the mapper reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCategory {
    New,
    Indeterminate,
    Done,
}

impl StatusCategory {
    /// The wire string: `new`, `indeterminate` or `done`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Indeterminate => "indeterminate",
            Self::Done => "done",
        }
    }
}

/// What Workgraph takes from one tracker issue.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackerSnapshot {
    pub external_id: String,
    pub key: String,
    pub url: String,
    pub project_key: String,
    pub title: String,
    pub description: String,
    pub priority: TaskPriority,
    pub assignee: Option<String>,
    pub status_name: String,
    pub status_category: StatusCategory,
    pub resolution: Option<String>,
    pub parent_key: Option<String>,
    /// Keys of issues that block this one (Jira "is blocked by" links), sorted, unique.
    pub blocked_by_keys: Vec<String>,
    pub pull_requests: Vec<PullRequestRef>,
    pub remote_updated_at: Timestamp,
    pub issue_type: String,
    pub labels: Vec<String>,
    /// The issue JSON as received.
    pub raw: Value,
}

/// Why an issue cannot be mapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// The issue has no `id` or no `key`.
    MissingField(&'static str),
    /// The issue has no `fields.updated` text.
    MissingUpdated,
    /// `fields.updated` is not an ISO-8601 timestamp.
    InvalidTimestamp(String),
}

impl MapError {
    /// The message of the Python `ValueError` this mirrors, where the message is part of the
    /// contract.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::MissingField(name) => format!("Jira issue has no {name}"),
            Self::MissingUpdated => "Jira issue has no fields.updated timestamp".to_string(),
            Self::InvalidTimestamp(text) => format!("invalid Jira timestamp {text:?}"),
        }
    }
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for MapError {}

/// Builds the snapshot for one issue from its JSON and its development-panel pull requests.
pub fn map_issue(
    issue: &Value,
    dev_pull_requests: &[Value],
    config: &MappingConfig,
) -> Result<TrackerSnapshot, MapError> {
    let fields = issue.get("fields").unwrap_or(&Value::Null);
    let key = identifier(issue.get("key")).ok_or(MapError::MissingField("key"))?;
    let external_id = identifier(issue.get("id")).ok_or(MapError::MissingField("id"))?;
    let project_key =
        text(dig(fields, &["project", "key"])).unwrap_or_else(|| project_of(&key).to_string());
    Ok(TrackerSnapshot {
        url: format!("{}/browse/{key}", config.base_url.trim_end_matches('/')),
        external_id,
        project_key,
        title: string_or_empty(fields.get("summary")),
        description: description_text(fields.get("description")),
        priority: priority(dig(fields, &["priority", "name"])),
        assignee: text(dig(fields, &["assignee", "displayName"])),
        status_name: string_or_empty(dig(fields, &["status", "name"])),
        status_category: status_category(dig(fields, &["status", "statusCategory", "key"])),
        resolution: text(dig(fields, &["resolution", "name"])),
        parent_key: text(dig(fields, &["parent", "key"])),
        blocked_by_keys: blocked_by_keys(fields.get("issuelinks")),
        pull_requests: pull_requests(fields, dev_pull_requests, config),
        remote_updated_at: remote_timestamp(fields.get("updated"))?,
        issue_type: string_or_empty(dig(fields, &["issuetype", "name"])),
        labels: labels(fields.get("labels")),
        raw: issue.clone(),
        key,
    })
}

/// Walks nested objects by key; `None` as soon as a step is not an object.
fn dig<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in keys {
        current = current.get(*key)?;
    }
    Some(current)
}

/// The text of a required identifier (`str(issue["key"])`): strings as they are, numbers as text.
fn identifier(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A non-empty string, or a number written as text. Anything else, and empty text, is `None`.
fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn string_or_empty(value: Option<&Value>) -> String {
    text(value).unwrap_or_default()
}

fn priority(name: Option<&Value>) -> TaskPriority {
    let Some(name) = name.and_then(Value::as_str) else {
        return TaskPriority::Medium;
    };
    match name.trim().to_lowercase().as_str() {
        "highest" | "blocker" | "critical" => TaskPriority::Critical,
        "high" | "major" => TaskPriority::High,
        "low" | "lowest" | "minor" | "trivial" => TaskPriority::Low,
        _ => TaskPriority::Medium,
    }
}

fn status_category(key: Option<&Value>) -> StatusCategory {
    match key
        .and_then(Value::as_str)
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("indeterminate") => StatusCategory::Indeterminate,
        Some("done") => StatusCategory::Done,
        _ => StatusCategory::New,
    }
}

fn blocked_by_keys(links: Option<&Value>) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for link in links.and_then(Value::as_array).into_iter().flatten() {
        if dig(link, &["type", "name"]).and_then(Value::as_str) != Some("Blocks") {
            continue;
        }
        if let Some(key) = link
            .get("inwardIssue")
            .and_then(|issue| text(issue.get("key")))
        {
            keys.insert(key);
        }
    }
    keys.into_iter().collect()
}

fn labels(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().map(label_text).collect())
        .unwrap_or_default()
}

fn label_text(label: &Value) -> String {
    match label {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn description_text(value: Option<&Value>) -> String {
    match value {
        Some(node @ Value::Object(_)) => adf_to_text(node),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Parses `fields.updated` into UTC. Jira writes `+0000` offsets; `Z` and `+00:00` also work.
fn remote_timestamp(value: Option<&Value>) -> Result<Timestamp, MapError> {
    let text = value
        .and_then(Value::as_str)
        .ok_or(MapError::MissingUpdated)?;
    Timestamp::parse_iso(&offset_with_colon(text))
        .map_err(|_| MapError::InvalidTimestamp(text.to_string()))
}

/// Inserts the colon of a trailing `+HHMM` or `-HHMM` offset, the form `parse_iso` reads.
fn offset_with_colon(text: &str) -> String {
    let len = text.len();
    let Some(tail) = text.get(len.saturating_sub(5)..) else {
        return text.to_string();
    };
    let mut bytes = tail.bytes();
    let sign_ok = matches!(bytes.next(), Some(b'+' | b'-'));
    if tail.len() == 5 && sign_ok && bytes.all(|b| b.is_ascii_digit()) {
        format!("{}:{}", &text[..len - 2], &text[len - 2..])
    } else {
        text.to_string()
    }
}

// --- pull requests --------------------------------------------------------------------------------

fn pull_requests(
    fields: &Value,
    dev_pull_requests: &[Value],
    config: &MappingConfig,
) -> Vec<PullRequestRef> {
    let mut found = Vec::new();
    for field_id in &config.pr_fields {
        let mut texts = Vec::new();
        collect_strings(fields.get(field_id.as_str()), &mut texts);
        for value_text in texts {
            for url in urls_in(value_text) {
                let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?']);
                if let Some(reference) = parse_pull_request_url(trimmed) {
                    found.push(PullRequestRef {
                        sources: vec![SOURCE_CUSTOM_FIELD],
                        ..reference
                    });
                }
            }
        }
    }
    if config.dev_panel {
        for entry in dev_pull_requests {
            let url = string_or_empty(entry.get("url"));
            if let Some(reference) = parse_pull_request_url(&url) {
                found.push(PullRequestRef {
                    sources: vec![SOURCE_DEV_PANEL],
                    title: string_or_empty(entry.get("name")),
                    ..reference
                });
            }
        }
    }
    merge(found)
}

/// Every string inside a field value: plain strings, lists, ADF trees and `{"url": ...}` objects.
fn collect_strings<'a>(value: Option<&'a Value>, out: &mut Vec<&'a str>) {
    match value {
        Some(Value::String(s)) => out.push(s),
        Some(Value::Object(map)) => {
            for item in map.values() {
                collect_strings(Some(item), out);
            }
        }
        Some(Value::Array(items)) => {
            for item in items {
                collect_strings(Some(item), out);
            }
        }
        _ => {}
    }
}
