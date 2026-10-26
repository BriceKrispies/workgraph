//! wg-trackers: Jira issue mapping, status mapping, sync decisions.
//!
//! Every function here is a pure decision over values: the issue JSON, the development-panel
//! entries, the mapping configuration and the task status. The Jira client, the configuration
//! file and the sync service stay in the shell (docs/rust-rewrite.md section 4.6). The golden
//! vectors of GV-08 (`golden/jira/`) and the port of `test_tracker_mapper.py` pin the behaviour.

mod adf;
mod mapping;
mod pull_request;
mod status;

pub use adf::adf_to_text;
pub use mapping::{MapError, MappingConfig, StatusCategory, TrackerSnapshot, map_issue};
pub use pull_request::{
    PullRequestRef, SOURCE_CUSTOM_FIELD, SOURCE_DEV_PANEL, parse_pull_request_url,
};
pub use status::tracker_target_status;
