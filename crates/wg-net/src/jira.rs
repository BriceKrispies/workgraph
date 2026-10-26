//! The read-only Jira client (`integrations/trackers/jira/client.py`). Only GET is sent. Requests,
//! pagination, JQL, the warnings and response reading are `wg-netspec`'s; this module sends,
//! sleeps and asks the system for the user's time zone.

use serde_json::Value;
use wg_netspec::TrackerError;
use wg_netspec::jira::{
    self as spec, DEV_STATUS_PATH, Flavour, NO_TIME_ZONE_WARNING, dev_status_warning,
    unknown_time_zone_warning,
};
use wg_netspec::jql;
use wg_netspec::retry::jira_step;

use crate::client::send_with_retries;
use crate::host::Net;

/// A read-only client for one Jira site. It sends only GET requests.
pub struct JiraClient {
    pub base_url: String,
    pub flavour: Flavour,
    pub timeout_seconds: f64,
    pub net: Net,
    /// The user's zone name, once read: `Some(None)` is UTC (no zone, or an unknown one).
    pub zone: Option<Option<String>>,
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
        let query_text = jql::updated_jql(projects, minute.as_deref());
        self.search_all(&query_text, "*all")
    }

    /// The `(id, key)` pair of every issue in `projects`.
    pub fn list_keys(
        &mut self,
        projects: &[String],
    ) -> Result<Vec<(String, String)>, TrackerError> {
        let query_text = jql::project_jql(projects);
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

    /// One GET with retries on 429 and 5xx, read as a JSON object.
    fn get(&mut self, path: &str, query: &[(String, String)]) -> Result<Value, TrackerError> {
        let call = spec::get_call(
            &self.base_url,
            &self.flavour,
            path,
            query,
            self.timeout_seconds,
        );
        let (result, _) = send_with_retries(&mut self.net, &call, jira_step);
        match result {
            Err(err) => Err(spec::transport_error(path, &err.message)),
            Ok(response) => spec::read_response(path, response.status, &response.body),
        }
    }
}
