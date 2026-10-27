//! The sync report of one `tracker sync` (`SyncReport` in `integrations/trackers/types.py`).
//!
//! The shell counts what it applies and fills this in; the JSON and human views are the
//! feature's. The cursor is epoch milliseconds, as the cursor rows store it.

/// What one sync did. Field order follows the Python dataclass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncReport {
    pub connection: String,
    pub full: bool,
    /// Issues returned by the listing, before references were fetched.
    pub fetched: i64,
    pub created: i64,
    pub updated: i64,
    pub unchanged: i64,
    /// Tracked issues a full sync no longer listed.
    pub missing: i64,
    pub archived: i64,
    /// Repository URLs of pull requests no registered repository matches, in first-seen order.
    pub unresolved_pull_requests: Vec<String>,
    pub errors: Vec<String>,
    /// The `updated` cursor after the sync, in epoch milliseconds, when one is set.
    pub cursor: Option<i64>,
}

impl SyncReport {
    /// An empty report for `connection`.
    #[must_use]
    pub fn new(connection: &str, full: bool) -> Self {
        SyncReport {
            connection: connection.to_owned(),
            full,
            ..SyncReport::default()
        }
    }
}
