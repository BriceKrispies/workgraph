//! Reads for external items (SH-14): the tracker mirror rows of tracked tasks.
//!
//! The statements are built by `wg_storecodec::external_sql` as values; this module runs them and
//! decodes the rows with the table codecs.

use wg_model::entity::ExternalItem;
use wg_storecodec::Raw;
use wg_storecodec::experiment_sql::int_scalar;
use wg_storecodec::external_sql;
use wg_types::{Id, WgError};

use crate::Session;
use crate::query::{many, one};

impl Session<'_> {
    /// The external item of `task_id`, if the task is tracked.
    pub fn external_item_by_task(&mut self, task_id: Id) -> Result<Option<ExternalItem>, WgError> {
        one(self, &external_sql::external_item_by_task(task_id))
    }

    /// The external item of `connection` with the tracker's id `external_id`, if there is one.
    pub fn external_item_by_external_id(
        &mut self,
        connection: &str,
        external_id: &str,
    ) -> Result<Option<ExternalItem>, WgError> {
        one(
            self,
            &external_sql::external_item_by_external_id(connection, external_id),
        )
    }

    /// Every external item of `connection`, ordered by the tracker's id.
    pub fn external_items_of_connection(
        &mut self,
        connection: &str,
    ) -> Result<Vec<ExternalItem>, WgError> {
        many(
            self,
            &external_sql::external_items_of_connection(connection),
        )
    }

    /// How many external items `connection` tracks.
    pub fn tracked_count(&mut self, connection: &str) -> Result<i64, WgError> {
        let rows = self.rows(&external_sql::tracked_count(connection))?;
        Ok(int_scalar(&rows).unwrap_or(0))
    }

    /// How many external items of `connection` a full sync no longer found.
    pub fn missing_count(&mut self, connection: &str) -> Result<i64, WgError> {
        let rows = self.rows(&external_sql::missing_count(connection))?;
        Ok(int_scalar(&rows).unwrap_or(0))
    }

    /// The distinct pull request repository names of `connection`'s tracked tasks that no
    /// repository row resolves, in name order.
    pub fn unresolved_pull_request_repos(
        &mut self,
        connection: &str,
    ) -> Result<Vec<String>, WgError> {
        self.rows(&external_sql::unresolved_pull_request_repos(connection))?
            .iter()
            .map(|values| match values.first() {
                Some(Raw::Text(name)) => Ok(name.clone()),
                _ => Err(wg_storecodec::db_error(
                    wg_storecodec::DbFailure::Other,
                    "a repository name row is not text",
                )),
            })
            .collect()
    }
}
