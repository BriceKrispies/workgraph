//! wg-trackers: Jira issue mapping, status mapping, sync decisions.
//!
//! Every function here is a pure decision over values: the issue JSON, the development-panel
//! entries, the mapping configuration, the task status and the task rows the shell supplies.
//! The Jira client, the configuration file and the sync service stay in the shell
//! (docs/rust-rewrite.md section 4.6). The golden vectors of GV-08 (`golden/jira/`), the port of
//! `test_tracker_mapper.py` and the oracle checks against `sync.py` pin the behaviour.

mod adf;
mod canonical;
mod mapping;
mod pull_request;
mod status;
mod sync;

pub use adf::adf_to_text;
pub use mapping::{MapError, MappingConfig, StatusCategory, TrackerSnapshot, map_issue};
pub use pull_request::{
    PullRequestRef, SOURCE_CUSTOM_FIELD, SOURCE_DEV_PANEL, parse_pull_request_url,
};
pub use status::tracker_target_status;
pub use sync::{
    BlockerTask, DependencyPlan, ExistingDependency, FieldUpdates, ParentFields, SyncLinks,
    TaskFields, dependency_order, dependency_plan, field_updates, project_of, snapshot_hash,
};
