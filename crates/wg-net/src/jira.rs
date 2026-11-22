//! The read-only Jira client (`integrations/trackers/jira/client.py`). Only GET is sent. Requests,
//! pagination, JQL, the warnings and response reading are `wg-netspec`'s; this module sends,
//! sleeps and asks the system for the user's time zone.

use serde_json::Value;
use wg_model::{EffectError, HttpResponse};
use wg_netspec::TrackerError;
use wg_netspec::jira::{
    self as spec, DEV_STATUS_PATH, Flavour, NO_TIME_ZONE_WARNING, dev_status_warning,
    unknown_time_zone_warning,
};
use wg_netspec::jql;
use wg_netspec::retry::{
    Attempt, JiraStep, header, jira_request_gap, jira_step, retry_after_too_long,
};

use crate::host::Net;

/// Where a connection's token comes from. Tokens are never logged or put in an error message.
pub trait TokenSource: Send {
    /// The current token.
    fn token(&mut self) -> Result<String, TrackerError>;
    /// Replaces the token after Jira rejected it; `Ok(false)` when there is nothing new to try.
    fn refresh(&mut self) -> Result<bool, TrackerError>;
}

/// A read-only client for one Jira site. It sends only GET requests.
pub struct JiraClient {
    pub base_url: String,
    /// The flavour, holding the token currently in use.
    pub flavour: Flavour,
    pub timeout_seconds: f64,
    pub net: Net,
    /// The user's zone name, once read: `Some(None)` is UTC (no zone, or an unknown one).
    pub zone: Option<Option<String>>,
    /// Where a new token comes from after a 401; `None` keeps the flavour's token.
    pub tokens: Option<Box<dyn TokenSource>>,
    /// Extra JQL ANDed onto every search.
    pub scope_jql: Option<String>,
    /// The smallest gap between two requests, in seconds.
    pub min_interval_seconds: f64,
    /// When the next request may go, on [`Net::monotonic_ms`]'s clock; 0 for a new client.
    pub next_request_ms: u64,
}

impl JiraClient {
    /// Every issue in `projects`, or only those updated since `since` (epoch seconds), oldest
    /// first. `since` is read in the user's own time zone, as JQL reads it.
    pub fn search_updated(
        &mut self,
        projects: &[String],
        since: Option<i64>,
    ) -> Result<Vec<Value>, TrackerError> {
        let minute = match since {
            Some(epoch) => {
                let offset = match self.user_zone()? {
                    Some(name) => self.net.zone_offset(&name, epoch),
                    None => 0,
                };
                Some(jql::jql_minute(epoch, offset))
            }
            None => None,
        };
        let query_text = jql::updated_jql(projects, self.scope_jql.as_deref(), minute.as_deref());
        self.search_all(&query_text, "*all")
    }

    /// The `(id, key)` pair of every issue in `projects`.
    pub fn list_keys(
        &mut self,
        projects: &[String],
    ) -> Result<Vec<(String, String)>, TrackerError> {
        let query_text = jql::scoped_jql(projects, self.scope_jql.as_deref());
        let fields = self.flavour.key_list_fields();
        let issues = self.search_all(&query_text, fields)?;
        Ok(spec::key_pairs(&issues))
    }

    /// One issue by key, or `None` when Jira answers 404.
    pub fn get_issue(&mut self, key: &str) -> Result<Option<Value>, TrackerError> {
        let path = spec::issue_path(&self.flavour, key);
        match self.get(&path, &[]) {
            Ok(issue) => Ok(Some(issue)),
            Err(err) if err.status == Some(404) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The pull requests linked to one issue in the development panel. Any failure reads as none,
    /// and is logged.
    pub fn dev_pull_requests(&mut self, external_id: &str) -> Vec<Value> {
        let query = spec::dev_status_query(external_id);
        match self.get(DEV_STATUS_PATH, &query) {
            Ok(body) => spec::dev_pull_requests(&body),
            Err(err) => {
                self.net
                    .warn(&dev_status_warning(external_id, &err.message));
                Vec::new()
            }
        }
    }

    /// Every page of a search, concatenated.
    fn search_all(&mut self, query_text: &str, fields: &str) -> Result<Vec<Value>, TrackerError> {
        let path = self.flavour.search_path();
        let mut cursor = self.flavour.first_cursor();
        let mut issues = Vec::new();
        loop {
            let query = self.flavour.search_query(query_text, fields, &cursor);
            let page = self.get(&path, &query)?;
            let page_issues = spec::issues_of_page(&page);
            let next = self.flavour.next_cursor(&cursor, &page, page_issues.len());
            issues.extend(page_issues);
            match next {
                Some(next) => cursor = next,
                None => return Ok(issues),
            }
        }
    }

    /// The user's zone name from `/myself`, read once and kept. A missing or unknown zone is `None`
    /// (UTC) with a warning. A failed request is not kept, so the next call asks again.
    fn user_zone(&mut self) -> Result<Option<String>, TrackerError> {
        if let Some(zone) = &self.zone {
            return Ok(zone.clone());
        }
        let myself_path = format!("{}/myself", self.flavour.api_path());
        let myself = self.get(&myself_path, &[])?;
        let zone = match spec::time_zone_name(&myself) {
            None => {
                self.net.warn(NO_TIME_ZONE_WARNING);
                None
            }
            Some(name) if self.net.zone_known(&name) => Some(name),
            Some(name) => {
                self.net.warn(&unknown_time_zone_warning(&name));
                None
            }
        };
        self.zone = Some(zone.clone());
        Ok(zone)
    }

    /// One GET, read as a JSON object. 429 and 5xx are retried, and a 401 is retried once with a
    /// new token when the token source has one.
    fn get(&mut self, path: &str, query: &[(String, String)]) -> Result<Value, TrackerError> {
        let mut attempt_no = 1;
        let mut refreshed = false;
        loop {
            self.pace();
            let call = spec::get_call(
                &self.base_url,
                &self.flavour,
                path,
                query,
                self.timeout_seconds,
            );
            let result = self.net.perform(&call);
            self.schedule_next(&result);
            let attempt = Attempt::of(&result);
            match jira_step(&attempt, attempt_no, refreshed) {
                JiraStep::Finished => return spec::read_result(path, result),
                JiraStep::Retry { delay_seconds } => {
                    self.net.pause(delay_seconds);
                    attempt_no += 1;
                }
                JiraStep::Reauthenticate => {
                    refreshed = true;
                    if !self.refresh_token()? {
                        return spec::read_result(path, result);
                    }
                }
                JiraStep::RetryAfterTooLong => {
                    let status = result.as_ref().map_or(0, |response| response.status);
                    return Err(retry_after_too_long(path, status));
                }
            }
        }
    }

    /// Asks the token source for a new token and puts it in the flavour.
    fn refresh_token(&mut self) -> Result<bool, TrackerError> {
        let Some(tokens) = self.tokens.as_mut() else {
            return Ok(false);
        };
        if !tokens.refresh()? {
            return Ok(false);
        }
        self.flavour = self.flavour.with_token(tokens.token()?);
        Ok(true)
    }

    /// Waits until the gap after the last response has passed.
    fn pace(&mut self) {
        let now = self.net.monotonic_ms();
        if self.next_request_ms > now {
            let wait_ms = self.next_request_ms - now;
            self.net.pause(wait_ms as f64 / 1000.0);
        }
    }

    /// When the next request may go, from the configured gap and the response's rate-limit hint.
    fn schedule_next(&mut self, result: &Result<HttpResponse, EffectError>) {
        let Ok(response) = result else {
            return;
        };
        let near_limit = header(&response.headers, "X-RateLimit-NearLimit");
        let gap = jira_request_gap(self.min_interval_seconds, near_limit.as_deref());
        if gap > self.min_interval_seconds {
            self.net.warn(&format!(
                "Jira reports the rate limit is near; pausing {gap}s between requests"
            ));
        }
        let gap_ms = (gap * 1000.0).round().max(0.0) as u64;
        self.next_request_ms = self.net.monotonic_ms().saturating_add(gap_ms);
    }
}
